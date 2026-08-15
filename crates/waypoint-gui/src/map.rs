//! The map panel: OSM tiles via `walkers`, with the track and current position
//! drawn on top as a plugin.

use std::collections::VecDeque;

use eframe::egui::{self, Align2, Color32, FontId, Pos2, Response, Shape, Stroke, Ui};
use walkers::{MapMemory, Plugin, Position, Projector, lat_lon};
use waypoint_core::{GnssState, TrackPoint};

pub const TRACK_COLOR: Color32 = Color32::from_rgb(0, 176, 240);
pub const FIX_COLOR: Color32 = Color32::from_rgb(255, 92, 92);

/// Minimum on-screen separation between retained track vertices. A long session
/// holds hundreds of thousands of fixes, most of which land on the same pixel at
/// any given zoom — drawing them all would cost far more than it shows.
const MIN_VERTEX_SPACING_PX: f32 = 2.0;

pub struct TrackPlugin<'a> {
    pub track: &'a VecDeque<TrackPoint>,
    pub current: Option<Position>,
    /// Direction the marker points: heading if the receiver reports one, else course.
    pub bearing_deg: Option<f32>,
    /// A stale position (no current fix) is drawn dimmed rather than hidden.
    pub has_fix: bool,
}

impl Plugin for TrackPlugin<'_> {
    fn run(
        self: Box<Self>,
        ui: &mut Ui,
        _response: &Response,
        projector: &Projector,
        _map_memory: &MapMemory,
    ) {
        let painter = ui.painter();

        let mut vertices: Vec<Pos2> = Vec::new();
        for point in self.track {
            let screen = projector.project(lat_lon(point.lat, point.lon)).to_pos2();
            if vertices
                .last()
                .is_none_or(|last| last.distance(screen) > MIN_VERTEX_SPACING_PX)
            {
                vertices.push(screen);
            }
        }

        if vertices.len() > 1 {
            painter.add(Shape::line(vertices, Stroke::new(3.0, TRACK_COLOR)));
        }

        let Some(current) = self.current else {
            return;
        };
        let center = projector.project(current).to_pos2();
        let color = if self.has_fix {
            FIX_COLOR
        } else {
            FIX_COLOR.gamma_multiply(0.4)
        };

        painter.circle_filled(center, 7.0, color);
        painter.circle_stroke(center, 7.0, Stroke::new(2.0, Color32::WHITE));

        // A course needle, so the map shows which way the receiver is travelling.
        if let Some(bearing) = self.bearing_deg {
            let radians = bearing.to_radians();
            let direction = egui::vec2(radians.sin(), -radians.cos());
            painter.line_segment(
                [center + direction * 8.0, center + direction * 26.0],
                Stroke::new(2.5, color),
            );
        }

        if !self.has_fix {
            painter.text(
                center + egui::vec2(12.0, -12.0),
                Align2::LEFT_BOTTOM,
                "no fix",
                FontId::proportional(11.0),
                color,
            );
        }
    }
}

/// Position the map should centre on: the live fix, else the last known point.
pub fn map_center(state: &GnssState) -> Position {
    state
        .position()
        .or_else(|| state.track.back().map(|p| (p.lat, p.lon)))
        .map(|(lat, lon)| lat_lon(lat, lon))
        .unwrap_or_else(|| lat_lon(0.0, 0.0))
}
