//! The eframe application: owns the engine, the map state and the source picker.

use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText};
use tokio::runtime::{Handle, Runtime};
use walkers::sources::OpenStreetMap;
use walkers::{HttpOptions, HttpTiles, Map, MapMemory};
use waypoint_core::{
    COMMON_BAUD_RATES, ConnectionStatus, Engine, FileMode, ReplaySpeed, SourceConfig, TripConfig,
    available_serial_ports,
};

use crate::map::{TrackPlugin, map_center};
use crate::panels;
use crate::skyview;

/// Tile servers identify clients by user agent; OpenStreetMap's usage policy
/// requires a real one. <https://operations.osmfoundation.org/policies/tiles/>
const USER_AGENT: &str = concat!("waypoint-gui/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    Serial,
    Tcp,
    File,
}

/// Deferred because the picker runs while the state read guard is held; the
/// engine can only be reconfigured once that guard is dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerAction {
    None,
    Connect,
    Disconnect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidePanel {
    Satellites,
    Plots,
    Raw,
}

/// Draft source settings, applied only when "Connect" is pressed.
struct SourceForm {
    kind: SourceKind,
    serial_port: String,
    baud: u32,
    ports: Vec<String>,
    tcp_addr: String,
    file_path: String,
    replay: bool,
    speed: f32,
}

impl Default for SourceForm {
    fn default() -> Self {
        let ports = available_serial_ports();
        Self {
            kind: SourceKind::Serial,
            serial_port: ports.first().cloned().unwrap_or_default(),
            baud: 9600,
            ports,
            tcp_addr: "127.0.0.1:10110".to_string(),
            file_path: String::new(),
            replay: true,
            speed: 4.0,
        }
    }
}

impl SourceForm {
    fn to_config(&self) -> Option<SourceConfig> {
        match self.kind {
            SourceKind::Serial => (!self.serial_port.is_empty()).then(|| SourceConfig::Serial {
                path: self.serial_port.clone(),
                baud: self.baud,
            }),
            SourceKind::Tcp => (!self.tcp_addr.is_empty()).then(|| SourceConfig::Tcp {
                addr: self.tcp_addr.clone(),
            }),
            SourceKind::File => (!self.file_path.is_empty()).then(|| SourceConfig::File {
                path: PathBuf::from(&self.file_path),
                mode: if self.replay {
                    FileMode::Replay {
                        speed: ReplaySpeed::new(self.speed),
                    }
                } else {
                    FileMode::Instant
                },
            }),
        }
    }

    /// Pre-fill the form from a source given on the command line.
    fn apply(&mut self, config: &SourceConfig) {
        match config {
            SourceConfig::Serial { path, baud } => {
                self.kind = SourceKind::Serial;
                self.serial_port = path.clone();
                self.baud = *baud;
            }
            SourceConfig::Tcp { addr } => {
                self.kind = SourceKind::Tcp;
                self.tcp_addr = addr.clone();
            }
            SourceConfig::File { path, mode } => {
                self.kind = SourceKind::File;
                self.file_path = path.display().to_string();
                match mode {
                    FileMode::Instant => self.replay = false,
                    FileMode::Replay { speed } => {
                        self.replay = true;
                        self.speed = speed.get();
                    }
                }
            }
        }
    }
}

pub struct WaypointApp {
    // Kept alive for the engine's tasks; all spawning goes through `handle`.
    _runtime: Runtime,
    handle: Handle,
    engine: Engine,

    tiles: HttpTiles,
    map_memory: MapMemory,
    follow: bool,

    form: SourceForm,
    side: SidePanel,
    show_source_picker: bool,
}

impl WaypointApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        runtime: Runtime,
        initial_source: Option<SourceConfig>,
    ) -> Self {
        let handle = runtime.handle().clone();
        let engine = Engine::new(TripConfig::default());

        let mut form = SourceForm::default();
        if let Some(config) = &initial_source {
            form.apply(config);
            let _guard = handle.enter();
            engine.set_source(config.clone());
        }

        // Repaint when new data lands rather than polling on a timer — a GPS
        // emits about once a second, and idling at 60 fps for that is wasteful.
        let ctx = cc.egui_ctx.clone();
        let mut updates = engine.updates();
        handle.spawn(async move {
            while updates.changed().await.is_ok() {
                ctx.request_repaint();
            }
        });

        let http_options = HttpOptions {
            user_agent: walkers::HeaderValue::from_static(USER_AGENT).into(),
            ..Default::default()
        };

        Self {
            handle,
            _runtime: runtime,
            engine,
            tiles: HttpTiles::with_options(OpenStreetMap, http_options, cc.egui_ctx.clone()),
            map_memory: MapMemory::default(),
            follow: true,
            form,
            side: SidePanel::Satellites,
            show_source_picker: initial_source.is_none(),
        }
    }

    fn connect(&mut self) {
        if let Some(config) = self.form.to_config() {
            let _guard = self.handle.enter();
            self.engine.set_source(config);
            self.show_source_picker = false;
            self.follow = true;
        }
    }

    fn source_picker(
        form: &mut SourceForm,
        live_speed: Option<&ReplaySpeed>,
        ui: &mut egui::Ui,
    ) -> PickerAction {
        let mut action = PickerAction::None;

        ui.horizontal(|ui| {
            ui.selectable_value(&mut form.kind, SourceKind::Serial, "Serial");
            ui.selectable_value(&mut form.kind, SourceKind::Tcp, "TCP");
            ui.selectable_value(&mut form.kind, SourceKind::File, "File");
        });
        ui.add_space(4.0);

        match form.kind {
            SourceKind::Serial => {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("serial_port")
                        .selected_text(if form.serial_port.is_empty() {
                            "no ports found".to_string()
                        } else {
                            form.serial_port.clone()
                        })
                        .show_ui(ui, |ui| {
                            for port in &form.ports {
                                ui.selectable_value(&mut form.serial_port, port.clone(), port);
                            }
                        });
                    if ui.button("⟳").on_hover_text("Rescan ports").clicked() {
                        form.ports = available_serial_ports();
                    }
                });
                egui::ComboBox::from_id_salt("baud")
                    .selected_text(format!("{} baud", form.baud))
                    .show_ui(ui, |ui| {
                        for baud in COMMON_BAUD_RATES {
                            ui.selectable_value(&mut form.baud, baud, baud.to_string());
                        }
                    });
            }
            SourceKind::Tcp => {
                ui.add(
                    egui::TextEdit::singleline(&mut form.tcp_addr)
                        .hint_text("host:port")
                        .desired_width(f32::INFINITY),
                );
            }
            SourceKind::File => {
                ui.horizontal(|ui| {
                    if ui.button("Choose file…").clicked()
                        && let Some(path) = rfd::FileDialog::new()
                            .add_filter("NMEA logs", &["nmea", "log", "txt"])
                            .add_filter("All files", &["*"])
                            .pick_file()
                    {
                        form.file_path = path.display().to_string();
                    }

                    // Only the file name fits the panel; the full path is on hover.
                    let label = std::path::Path::new(&form.file_path)
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "no file selected".to_string());
                    ui.label(RichText::new(label).size(11.0).weak())
                        .on_hover_text(&form.file_path);
                });
                ui.checkbox(&mut form.replay, "Replay in original timing");
                if ui
                    .add_enabled(
                        form.replay,
                        egui::Slider::new(&mut form.speed, ReplaySpeed::MIN..=ReplaySpeed::MAX)
                            .logarithmic(true)
                            .text("× speed"),
                    )
                    .changed()
                    && let Some(live) = live_speed
                {
                    // Apply to the replay already running, rather than waiting
                    // for a reconnect that would discard the track.
                    live.set(form.speed);
                }
            }
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("Connect").clicked() {
                action = PickerAction::Connect;
            }
            if ui.button("Disconnect").clicked() {
                action = PickerAction::Disconnect;
            }
        });

        action
    }
}

impl eframe::App for WaypointApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // One read guard per frame; the ingest task never holds the write lock
        // across an await, so this cannot stall the UI.
        let state = self.engine.state();
        let live_speed = self.engine.replay_speed();
        let mut picker_action = PickerAction::None;

        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("WAYPOINT").strong().size(16.0));
                ui.separator();

                let connection_color = match &state.connection {
                    ConnectionStatus::Connected => Color32::from_rgb(110, 200, 120),
                    ConnectionStatus::Connecting | ConnectionStatus::Reconnecting { .. } => {
                        Color32::from_rgb(230, 180, 60)
                    }
                    ConnectionStatus::Idle => Color32::from_gray(140),
                    _ => Color32::from_rgb(220, 80, 80),
                };
                ui.label(RichText::new("●").color(connection_color));
                ui.label(state.connection.label());
                if !state.source_label.is_empty() {
                    ui.weak(&state.source_label);
                }

                ui.separator();
                ui.label(
                    RichText::new(state.fix_mode.label())
                        .strong()
                        .color(panels::fix_color(state.fix_mode)),
                );

                if let Some(speed) = &live_speed {
                    ui.separator();
                    ui.label(
                        RichText::new(format!("replay ×{:.2}", speed.get()))
                            .size(12.0)
                            .color(Color32::from_rgb(226, 112, 214)),
                    )
                    .on_hover_text("Replay rate — adjustable live from the Source panel");
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Source…").clicked() {
                        self.show_source_picker = !self.show_source_picker;
                    }
                    if ui.button("Reset trip").clicked() {
                        self.engine.reset_trip();
                    }
                    ui.toggle_value(&mut self.follow, "Follow");
                });
            });
        });

        egui::Panel::left("status")
            .default_size(300.0)
            .show(ui, |ui| {
                if self.show_source_picker {
                    ui.add_space(4.0);
                    ui.label(RichText::new("Source").strong());
                    picker_action = Self::source_picker(&mut self.form, live_speed.as_ref(), ui);
                    ui.separator();
                }

                egui::ScrollArea::vertical().show(ui, |ui| {
                    panels::status_panel(ui, &state);
                    ui.add_space(8.0);
                    ui.label(RichText::new("Trip").strong());
                    panels::trip_panel(ui, &state);
                    ui.add_space(8.0);
                    panels::counters_bar(ui, &state);
                });
            });

        egui::Panel::right("detail")
            .default_size(360.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.side, SidePanel::Satellites, "Satellites");
                    ui.selectable_value(&mut self.side, SidePanel::Plots, "Plots");
                    ui.selectable_value(&mut self.side, SidePanel::Raw, "Raw");
                });
                ui.separator();

                match self.side {
                    SidePanel::Satellites => {
                        let size = ui.available_width().min(320.0);
                        skyview::sky_view(ui, &state, size);
                        ui.add_space(6.0);
                        ui.label(RichText::new("Signal strength (dB-Hz)").weak().size(11.0));
                        skyview::signal_bars(ui, &state);
                    }
                    SidePanel::Plots => panels::plots_panel(ui, &state),
                    SidePanel::Raw => panels::raw_panel(ui, &state),
                }
            });

        egui::CentralPanel::default().show(ui, |ui| {
            if self.follow && !self.map_memory.animating() {
                self.map_memory.follow_my_position();
            }

            let plugin = TrackPlugin {
                track: &state.track,
                current: state
                    .position()
                    .map(|(lat, lon)| walkers::lat_lon(lat, lon)),
                bearing_deg: state.heading_deg.or(state.course_deg),
                has_fix: state.fix_mode.has_fix() && state.fix_quality.is_valid(),
            };

            let map = Map::new(
                Some(&mut self.tiles),
                &mut self.map_memory,
                map_center(&state),
            )
            .with_plugin(plugin);

            ui.add(map);

            // Dragging the map detaches it from the fix; reflect that in the toggle
            // rather than fighting the user for the centre.
            if self.map_memory.detached().is_some() {
                self.follow = false;
            }

            map_overlay(ui, &mut self.map_memory, &self.tiles);
        });

        drop(state);
        match picker_action {
            PickerAction::Connect => self.connect(),
            PickerAction::Disconnect => self.engine.stop(),
            PickerAction::None => {}
        }
    }
}

/// Zoom buttons and the tile attribution, drawn over the map.
fn map_overlay(ui: &mut egui::Ui, memory: &mut MapMemory, tiles: &HttpTiles) {
    let rect = ui.min_rect();

    egui::Area::new("zoom".into())
        .fixed_pos(rect.left_top() + egui::vec2(10.0, 10.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui.button(RichText::new("+").size(16.0)).clicked() {
                        let _ = memory.zoom_in();
                    }
                    if ui.button(RichText::new("−").size(16.0)).clicked() {
                        let _ = memory.zoom_out();
                    }
                    ui.label(format!("z{:.0}", memory.zoom()));
                });
            });
        });

    let attribution = walkers::Tiles::attribution(tiles);
    egui::Area::new("attribution".into())
        .fixed_pos(rect.right_bottom() - egui::vec2(240.0, 24.0))
        .show(ui.ctx(), |ui| {
            ui.hyperlink_to(RichText::new(attribution.text).size(10.0), attribution.url);
        });
}
