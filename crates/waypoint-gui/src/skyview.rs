//! Polar sky view: satellites placed by azimuth and elevation, as seen from the
//! receiver. Painted directly rather than via `egui_plot` — a polar layout with
//! compass labels is simpler to draw than to coerce out of a cartesian plot.

use eframe::egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use waypoint_core::{Constellation, GnssState};

pub fn constellation_color(constellation: Constellation) -> Color32 {
    match constellation {
        Constellation::Gps => Color32::from_rgb(0, 176, 240),
        Constellation::Glonass => Color32::from_rgb(226, 112, 214),
        Constellation::Galileo => Color32::from_rgb(120, 200, 120),
        Constellation::Beidou => Color32::from_rgb(240, 190, 70),
        Constellation::Qzss => Color32::from_rgb(150, 150, 240),
        Constellation::NavIC => Color32::from_rgb(240, 140, 100),
    }
}

/// Colour a signal strength the way a receiver's own bar display would.
pub fn snr_color(snr: Option<f32>) -> Color32 {
    match snr {
        None | Some(0.0) => Color32::from_gray(90),
        Some(s) if s < 25.0 => Color32::from_rgb(220, 90, 90),
        Some(s) if s < 35.0 => Color32::from_rgb(230, 190, 80),
        Some(_) => Color32::from_rgb(110, 200, 120),
    }
}

pub fn sky_view(ui: &mut Ui, state: &GnssState, size: f32) {
    let (response, painter) = ui.allocate_painter(Vec2::splat(size), Sense::hover());
    let rect = response.rect;
    let center = rect.center();
    let radius = (rect.width().min(rect.height()) / 2.0) - 14.0;

    let grid = ui.visuals().weak_text_color().gamma_multiply(0.6);
    let label_color = ui.visuals().weak_text_color();

    // Elevation rings: horizon, 30° and 60°, with the zenith at the centre.
    for elevation in [0.0, 30.0, 60.0] {
        painter.circle_stroke(
            center,
            elevation_radius(radius, elevation),
            Stroke::new(1.0, grid),
        );
    }
    painter.line_segment(
        [
            center - Vec2::new(radius, 0.0),
            center + Vec2::new(radius, 0.0),
        ],
        Stroke::new(1.0, grid),
    );
    painter.line_segment(
        [
            center - Vec2::new(0.0, radius),
            center + Vec2::new(0.0, radius),
        ],
        Stroke::new(1.0, grid),
    );

    let font = FontId::proportional(11.0);
    for (label, azimuth) in [("N", 0.0), ("E", 90.0), ("S", 180.0), ("W", 270.0)] {
        let position = polar(center, radius + 9.0, azimuth);
        painter.text(
            position,
            Align2::CENTER_CENTER,
            label,
            font.clone(),
            label_color,
        );
    }

    for satellite in &state.satellites {
        let (Some(elevation), Some(azimuth)) = (satellite.elevation, satellite.azimuth) else {
            continue;
        };

        let position = polar(center, elevation_radius(radius, elevation), azimuth);
        let color = snr_color(satellite.snr);
        let marker = 4.0 + satellite.snr.unwrap_or(0.0) / 18.0;

        // Filled means the receiver is actually using it in the solution;
        // outlined means in view but not contributing.
        if satellite.used_in_fix {
            painter.circle_filled(position, marker, color);
        } else {
            painter.circle_stroke(position, marker, Stroke::new(1.5, color));
        }

        painter.text(
            position + Vec2::new(0.0, marker + 7.0),
            Align2::CENTER_CENTER,
            satellite.prn.to_string(),
            FontId::proportional(9.0),
            constellation_color(satellite.constellation),
        );
    }

    if state.satellites.is_empty() {
        painter.text(
            center,
            Align2::CENTER_CENTER,
            "no satellites reported",
            font,
            label_color,
        );
    }
}

/// Zenith (90°) at the centre, horizon (0°) at the rim.
fn elevation_radius(radius: f32, elevation: f32) -> f32 {
    radius * (1.0 - elevation.clamp(0.0, 90.0) / 90.0)
}

/// Azimuth is clockwise from north, and north is up.
fn polar(center: Pos2, radius: f32, azimuth_deg: f32) -> Pos2 {
    let radians = azimuth_deg.to_radians();
    center + Vec2::new(radians.sin() * radius, -radians.cos() * radius)
}

/// Per-constellation signal bars — the count-and-strength summary that the
/// sky view's geometry doesn't make easy to read at a glance.
pub fn signal_bars(ui: &mut Ui, state: &GnssState) {
    let available = ui.available_width();
    let count = state.satellites.len().max(1);
    let bar_width = ((available - 4.0) / count as f32).clamp(3.0, 18.0);
    let height = 46.0;

    let (response, painter) =
        ui.allocate_painter(Vec2::new(available, height + 12.0), Sense::hover());
    let rect = response.rect;

    for (index, satellite) in state.satellites.iter().enumerate() {
        let snr = satellite.snr.unwrap_or(0.0);
        let fraction = (snr / 50.0).clamp(0.0, 1.0);
        let x = rect.left() + 2.0 + index as f32 * bar_width;
        let bar = Rect::from_min_max(
            Pos2::new(x, rect.top() + height * (1.0 - fraction)),
            Pos2::new(x + bar_width - 1.5, rect.top() + height),
        );

        let color = snr_color(satellite.snr);
        painter.rect_filled(bar, 1.0, color);
        if !satellite.used_in_fix {
            // Not in the solution: hollow it out so the used ones stand out.
            painter.rect_filled(bar.shrink(1.5), 1.0, ui.visuals().panel_fill);
        }
        painter.text(
            Pos2::new(bar.center().x, rect.top() + height + 6.0),
            Align2::CENTER_CENTER,
            satellite.prn.to_string(),
            FontId::proportional(8.0),
            constellation_color(satellite.constellation),
        );
    }
}
