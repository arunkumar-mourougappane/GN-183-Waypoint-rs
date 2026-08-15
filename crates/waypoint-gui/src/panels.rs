//! Status, plots, trip statistics and the raw sentence view.

use eframe::egui::{self, Color32, Grid, RichText, Ui};
use egui_plot::{Legend, Line, Plot, PlotPoints};
use waypoint_core::{
    FixMode, GnssState, KNOTS_TO_KMH, KNOTS_TO_MPH, MetricSample, SentenceOutcome, hdop_rating,
};

use crate::skyview::constellation_color;

pub fn fix_color(mode: FixMode) -> Color32 {
    match mode {
        FixMode::NoFix => Color32::from_rgb(220, 80, 80),
        FixMode::Fix2D => Color32::from_rgb(230, 180, 60),
        FixMode::Fix3D => Color32::from_rgb(110, 200, 120),
    }
}

pub fn status_panel(ui: &mut Ui, state: &GnssState) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(state.fix_mode.label())
                .size(22.0)
                .strong()
                .color(fix_color(state.fix_mode)),
        );
        ui.vertical(|ui| {
            ui.add_space(2.0);
            ui.label(RichText::new(state.fix_quality.label()).size(13.0));
            ui.label(
                RichText::new(format!(
                    "{} used / {} in view",
                    state
                        .sats_used
                        .map(|n| n.to_string())
                        .unwrap_or_else(|| "-".into()),
                    state.sats_in_view()
                ))
                .size(11.0)
                .weak(),
            );
        });
    });

    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        for (constellation, satellites) in state.satellites_by_constellation() {
            let used = satellites.iter().filter(|s| s.used_in_fix).count();
            ui.label(
                RichText::new(format!(
                    "{} {}/{}",
                    constellation.short(),
                    used,
                    satellites.len()
                ))
                .size(11.0)
                .color(constellation_color(constellation)),
            )
            .on_hover_text(format!(
                "{}: {} in view, {} used in the fix",
                constellation.label(),
                satellites.len(),
                used
            ));
        }
    });

    ui.separator();

    Grid::new("fix_values")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            let position = state
                .position()
                .map(|(lat, lon)| format!("{lat:.6}, {lon:.6}"))
                .unwrap_or_else(|| "—".into());
            row(ui, "Position", &position);
            row(
                ui,
                "Altitude",
                &optional(state.altitude_m, |a| format!("{a:.1} m MSL")),
            );
            row(
                ui,
                "Speed",
                &optional(state.speed_knots, |s| {
                    format!(
                        "{s:.1} kt · {:.1} km/h · {:.1} mph",
                        s * KNOTS_TO_KMH,
                        s * KNOTS_TO_MPH
                    )
                }),
            );
            row(
                ui,
                "Course",
                &optional(state.course_deg, |c| format!("{c:.1}° true")),
            );
            row(
                ui,
                "Heading",
                &optional(state.heading_deg, |h| format!("{h:.1}°")),
            );
            row(
                ui,
                "HDOP",
                &optional(state.hdop, |h| format!("{h:.2}  ({})", hdop_rating(h))),
            );
            row(
                ui,
                "PDOP / VDOP",
                &format!(
                    "{} / {}",
                    optional(state.pdop, |v| format!("{v:.2}")),
                    optional(state.vdop, |v| format!("{v:.2}"))
                ),
            );
            row(
                ui,
                "Geoid sep.",
                &optional(state.geoid_separation_m, |g| format!("{g:.1} m")),
            );
            row(
                ui,
                "UTC",
                &optional(state.fix_time, |t| t.format("%H:%M:%S%.3f").to_string()),
            );
            row(
                ui,
                "Date",
                &optional(state.fix_date, |d| d.format("%Y-%m-%d").to_string()),
            );
        });
}

pub fn trip_panel(ui: &mut Ui, state: &GnssState) {
    let trip = &state.trip;

    Grid::new("trip_values")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            row(
                ui,
                "Distance",
                &format!("{:.3} km", trip.distance_m / 1000.0),
            );
            row(ui, "Elapsed", &format_duration(trip.elapsed_secs()));
            row(ui, "Moving", &format_duration(trip.moving_secs));
            row(ui, "Stopped", &format_duration(trip.stopped_secs()));
            row(
                ui,
                "Avg (moving)",
                &optional(trip.avg_moving_speed_knots(), |s| {
                    format!("{s:.1} kt · {:.1} km/h", s * KNOTS_TO_KMH)
                }),
            );
            row(
                ui,
                "Avg (overall)",
                &optional(trip.avg_overall_speed_knots(), |s| format!("{s:.1} kt")),
            );
            row(ui, "Max speed", &format!("{:.1} kt", trip.max_speed_knots));
            row(
                ui,
                "Elevation",
                &format!(
                    "+{:.0} m / −{:.0} m",
                    trip.elevation_gain_m, trip.elevation_loss_m
                ),
            );
            row(
                ui,
                "Avg HDOP",
                &optional(trip.avg_hdop(), |h| format!("{h:.2}")),
            );
            row(
                ui,
                "Avg sats",
                &optional(trip.avg_sats_used(), |s| format!("{s:.1}")),
            );
            row(ui, "Fixes", &trip.fix_count.to_string());
            row(ui, "Rejected", &format!("{} jump(s)", trip.rejected_jumps));
        });
}

/// Speed, altitude and HDOP against a shared time axis, so a spike in one is
/// easy to line up against the others.
pub fn plots_panel(ui: &mut Ui, state: &GnssState) {
    let Some(first) = state.history.front().map(|s| s.timestamp) else {
        ui.weak("Waiting for data…");
        return;
    };

    let elapsed =
        |sample: &MetricSample| (sample.timestamp - first).num_milliseconds() as f64 / 1000.0;
    let height = (ui.available_height() / 3.0).max(90.0);

    let series = |pick: fn(&MetricSample) -> Option<f32>| -> PlotPoints<'static> {
        state
            .history
            .iter()
            .filter_map(|sample| pick(sample).map(|value| [elapsed(sample), value as f64]))
            .collect()
    };

    for (name, unit, color, points) in [
        (
            "Speed",
            "kt",
            Color32::from_rgb(0, 176, 240),
            series(|s| s.speed_knots),
        ),
        (
            "Altitude",
            "m",
            Color32::from_rgb(110, 200, 120),
            series(|s| s.altitude_m),
        ),
        (
            "HDOP",
            "",
            Color32::from_rgb(230, 180, 60),
            series(|s| s.hdop),
        ),
    ] {
        Plot::new(format!("plot_{name}"))
            .height(height - 6.0)
            .legend(Legend::default())
            .allow_drag(true)
            .allow_scroll(false)
            .x_axis_label("s")
            .y_axis_label(unit)
            .show(ui, |plot_ui| {
                plot_ui.line(Line::new(name, points).color(color).width(1.5));
            });
    }
}

/// The raw stream, tagged with what the parser made of each line. Without this a
/// GPS debugger is just a tracker.
pub fn raw_panel(ui: &mut Ui, state: &GnssState) {
    egui::ScrollArea::vertical()
        .stick_to_bottom(true)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for raw in &state.raw {
                let (tag, color) = match &raw.outcome {
                    SentenceOutcome::Decoded(kind) => {
                        (kind.clone(), Color32::from_rgb(110, 200, 120))
                    }
                    SentenceOutcome::ChecksumFailed => {
                        ("CRC".to_string(), Color32::from_rgb(220, 80, 80))
                    }
                    SentenceOutcome::Unsupported => ("SKIP".to_string(), Color32::from_gray(120)),
                    SentenceOutcome::ParseError(_) => {
                        ("ERR".to_string(), Color32::from_rgb(240, 140, 100))
                    }
                };

                let response = ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("{tag:<5}")).monospace().color(color));
                    ui.label(RichText::new(&raw.text).monospace().size(11.0));
                });

                if let SentenceOutcome::ParseError(message) = &raw.outcome {
                    response.response.on_hover_text(message);
                }
            }
        });
}

pub fn counters_bar(ui: &mut Ui, state: &GnssState) {
    let counters = &state.counters;
    ui.horizontal(|ui| {
        ui.label(RichText::new(format!("{} sentences", counters.total)).size(11.0));
        ui.label(
            RichText::new(format!("{} decoded", counters.decoded))
                .size(11.0)
                .color(Color32::from_rgb(110, 200, 120)),
        );
        if counters.checksum_failed > 0 {
            ui.label(
                RichText::new(format!("{} checksum", counters.checksum_failed))
                    .size(11.0)
                    .color(Color32::from_rgb(220, 80, 80)),
            );
        }
        if counters.parse_failed > 0 {
            ui.label(
                RichText::new(format!("{} parse errors", counters.parse_failed))
                    .size(11.0)
                    .color(Color32::from_rgb(240, 140, 100)),
            );
        }
        if counters.unsupported > 0 {
            ui.label(
                RichText::new(format!("{} unsupported", counters.unsupported))
                    .size(11.0)
                    .weak(),
            );
        }
    });

    ui.horizontal_wrapped(|ui| {
        for (kind, count) in &counters.by_type {
            ui.label(RichText::new(format!("{kind} ×{count}")).size(10.0).weak());
        }
    });
}

fn row(ui: &mut Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).weak().size(12.0));
    ui.label(RichText::new(value).size(12.0));
    ui.end_row();
}

fn optional<T>(value: Option<T>, format: impl Fn(T) -> String) -> String {
    value.map(format).unwrap_or_else(|| "—".to_string())
}

fn format_duration(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        (total / 60) % 60,
        total % 60
    )
}
