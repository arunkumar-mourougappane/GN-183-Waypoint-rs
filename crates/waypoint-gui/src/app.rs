//! The eframe application: owns the engine, the map state and the source picker.

use std::path::PathBuf;

use eframe::egui::{self, Color32, RichText};
use tokio::runtime::{Handle, Runtime};
use walkers::sources::OpenStreetMap;
use walkers::{HttpOptions, HttpTiles, Map, MapMemory};
use waypoint_core::{
    COMMON_BAUD_RATES, Engine, FileMode, RecordingStatus, ReplayControl, SourceConfig,
    SourceControls, StatusTone, TripConfig, available_serial_ports,
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

/// Deferred for the same reason as the picker's: the state read guard is held
/// while the bar is drawn, and starting a capture is async.
#[derive(Debug, Clone, PartialEq)]
enum RecordAction {
    None,
    Start(PathBuf),
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidePanel {
    Satellites,
    Plots,
    Raw,
}

/// Draft source settings, applied only when the form is submitted.
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
            speed: 1.0,
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
                        control: ReplayControl::new(self.speed),
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
                    FileMode::Replay { control } => {
                        self.replay = true;
                        self.speed = control.speed();
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
    /// Where the scrub handle is being held, while it is being held.
    scrub: Option<f32>,
}

impl WaypointApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        runtime: Runtime,
        initial_source: Option<SourceConfig>,
        initial_recording: Option<PathBuf>,
    ) -> Self {
        let handle = runtime.handle().clone();
        let engine = Engine::new(TripConfig::default());

        let mut form = SourceForm::default();
        if let Some(config) = &initial_source {
            form.apply(config);
            let _guard = handle.enter();
            engine.set_source(config.clone());
        }

        if let Some(path) = initial_recording {
            let _guard = handle.enter();
            if let Err(err) = handle.block_on(engine.start_recording(&path)) {
                // Loud, not silent: someone who asked for a capture needs to
                // know it is not happening.
                tracing::error!(%err, "could not start recording");
            }
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
            scrub: None,
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
        live_control: Option<&ReplayControl>,
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
                        egui::Slider::new(
                            &mut form.speed,
                            ReplayControl::MIN_SPEED..=ReplayControl::MAX_SPEED,
                        )
                        .logarithmic(true)
                        .text("× speed"),
                    )
                    .changed()
                    && let Some(live) = live_control
                {
                    // Apply to the replay already running, rather than waiting
                    // for a reconnect that would discard the track.
                    live.set_speed(form.speed);
                }
            }
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            // A file is opened and closed; only a receiver is connected to.
            let (start, stop) = match form.kind {
                SourceKind::File => ("Open", "Close"),
                SourceKind::Serial | SourceKind::Tcp => ("Connect", "Disconnect"),
            };
            if ui.button(start).clicked() {
                action = PickerAction::Connect;
            }
            if ui.button(stop).clicked() {
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
        let controls = self.engine.controls();
        let live_control = self.engine.replay_control();
        let mut picker_action = PickerAction::None;
        let mut restart = false;
        let mut record_action = RecordAction::None;
        let recording = self.engine.recording();

        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("WAYPOINT").strong().size(16.0));
                ui.separator();

                let connection_color = match state.connection.tone(controls) {
                    StatusTone::Good => Color32::from_rgb(110, 200, 120),
                    StatusTone::Warning => Color32::from_rgb(230, 180, 60),
                    StatusTone::Bad => Color32::from_rgb(220, 80, 80),
                    StatusTone::Neutral => Color32::from_gray(140),
                };
                ui.label(RichText::new("●").color(connection_color));
                ui.label(state.connection.label_for(controls));
                if !state.source_label.is_empty() {
                    ui.weak(&state.source_label);
                }

                ui.separator();
                ui.label(
                    RichText::new(state.fix_mode.label())
                        .strong()
                        .color(panels::fix_color(state.fix_mode)),
                );

                match controls {
                    // A recording gets transport: play state, position, restart.
                    Some(SourceControls::Replay) => {
                        if let Some(control) = &live_control {
                            ui.separator();
                            transport_bar(ui, control, &mut restart, &mut self.scrub);
                        }
                    }
                    // A receiver gets liveness: is data still arriving, how fast.
                    // And a capture control, since it is the only kind of source
                    // that produces something worth keeping.
                    Some(SourceControls::Live) => {
                        ui.separator();
                        live_bar(ui, &state);
                        ui.separator();
                        record_bar(ui, recording.as_ref(), &mut record_action);
                    }
                    Some(SourceControls::Instant) | None => {}
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
                    picker_action = Self::source_picker(&mut self.form, live_control.as_ref(), ui);
                    ui.separator();
                }

                egui::ScrollArea::vertical().show(ui, |ui| {
                    panels::status_panel(ui, &state, controls);
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
        if restart {
            self.engine.restart();
        }
        match record_action {
            RecordAction::Start(path) => {
                let engine = &self.engine;
                let _guard = self.handle.enter();
                // Blocking briefly on file creation keeps the failure attached
                // to the click that caused it.
                if let Err(err) = self.handle.block_on(engine.start_recording(&path)) {
                    tracing::warn!(%err, "could not start recording");
                }
            }
            RecordAction::Stop => self.engine.stop_recording(),
            RecordAction::None => {}
        }
        match picker_action {
            PickerAction::Connect => self.connect(),
            PickerAction::Disconnect => self.engine.stop(),
            PickerAction::None => {}
        }
    }
}

/// Play/pause, restart, rate and position — the controls a finite recording can
/// honour and a live receiver cannot.
fn transport_bar(
    ui: &mut egui::Ui,
    control: &ReplayControl,
    restart: &mut bool,
    scrub: &mut Option<f32>,
) {
    let paused = control.is_paused();
    let (glyph, hint) = if paused {
        ("▶", "Resume")
    } else {
        ("⏸", "Pause")
    };
    if ui
        .button(RichText::new(glyph).size(14.0))
        .on_hover_text(hint)
        .clicked()
    {
        control.toggle_paused();
    }
    if ui
        .button(RichText::new("⟲").size(14.0))
        .on_hover_text("Restart from the beginning")
        .clicked()
    {
        *restart = true;
    }

    if ui.small_button("½×").on_hover_text("Half speed").clicked() {
        control.scale_speed(0.5);
    }
    ui.label(
        RichText::new(format!("×{:.2}", control.speed()))
            .size(12.0)
            .color(Color32::from_rgb(226, 112, 214)),
    );
    if ui
        .small_button("2×")
        .on_hover_text("Double speed")
        .clicked()
    {
        control.scale_speed(2.0);
    }

    if let Some(position) = control.progress() {
        // While the handle is held, the slider shows where the user has dragged
        // to; playback would otherwise drag it back under their finger.
        let mut shown = scrub.unwrap_or(position);
        ui.spacing_mut().slider_width = 180.0;
        let response = ui.add(
            egui::Slider::new(&mut shown, 0.0..=1.0)
                .show_value(false)
                .trailing_fill(true),
        );

        if response.drag_started() || response.changed() {
            *scrub = Some(shown);
        }
        if response.changed() {
            control.seek_to(shown);
        }
        // Hand control back to playback once released.
        if response.drag_stopped() || (!response.dragged() && !response.has_focus()) {
            *scrub = None;
        }

        ui.label(
            RichText::new(format!("{:>3.0}%", shown * 100.0))
                .size(12.0)
                .monospace(),
        )
        .on_hover_text("Position in the log — drag to seek");
    }

    if paused {
        ui.label(
            RichText::new("PAUSED")
                .size(12.0)
                .strong()
                .color(Color32::from_rgb(230, 180, 60)),
        );
    }
}

/// Liveness of a receiver: the readout that tells you the link is alive even
/// when the fix is not moving.
fn live_bar(ui: &mut egui::Ui, state: &waypoint_core::GnssState) {
    let health = &state.health;

    if health.is_stale() {
        let since = health.since_last_sentence().unwrap_or_default();
        ui.label(
            RichText::new(format!("⚠ no data for {:.0}s", since.as_secs_f32()))
                .size(12.0)
                .strong()
                .color(Color32::from_rgb(220, 80, 80)),
        )
        .on_hover_text("The receiver has gone quiet — check the cable, baud rate or link");
    } else {
        ui.label(
            RichText::new(format!("{:.1} msg/s", health.sentences_per_sec))
                .size(12.0)
                .color(Color32::from_rgb(0, 176, 240)),
        )
        .on_hover_text("Sentences arriving per second");
    }
}

/// Capture controls. Live sources only: a recorded log is already what this
/// would produce, so offering it there would just copy a file.
fn record_bar(ui: &mut egui::Ui, recording: Option<&RecordingStatus>, action: &mut RecordAction) {
    match recording {
        Some(capture) if capture.failed => {
            ui.label(
                RichText::new("● REC FAILED")
                    .size(12.0)
                    .strong()
                    .color(Color32::from_rgb(220, 80, 80)),
            )
            .on_hover_text(format!("{} could not be written", capture.path.display()));
            if ui.button("Stop").clicked() {
                *action = RecordAction::Stop;
            }
        }
        Some(capture) => {
            ui.label(
                RichText::new(format!("● REC {}", capture.size_label()))
                    .size(12.0)
                    .strong()
                    .color(Color32::from_rgb(220, 80, 80)),
            )
            .on_hover_text(format!(
                "{} — {} sentences{}",
                capture.path.display(),
                capture.sentences,
                if capture.dropped > 0 {
                    format!(", {} dropped", capture.dropped)
                } else {
                    String::new()
                }
            ));
            if ui.button("Stop").clicked() {
                *action = RecordAction::Stop;
            }
        }
        None => {
            if ui
                .button("● Record")
                .on_hover_text("Capture this stream to a log you can replay")
                .clicked()
                && let Some(path) = rfd::FileDialog::new()
                    .set_file_name(waypoint_core::record::default_filename(chrono::Utc::now()))
                    .add_filter("NMEA logs", &["nmea"])
                    .save_file()
            {
                *action = RecordAction::Start(path);
            }
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
