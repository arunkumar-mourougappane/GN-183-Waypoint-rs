//! Dashboard rendering. Panels mirror the GUI's breakdown — track, status,
//! parameter plots, satellites, trip stats — within what a terminal can draw.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::canvas::{Canvas, Points};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Sparkline, Table};
use waypoint_core::{
    Constellation, FixMode, GnssState, KNOTS_TO_KMH, SentenceOutcome, StatusTone, hdop_rating,
};

use crate::Transport;

/// What the lower panel shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BottomPanel {
    Plots,
    RawSentences,
}

impl BottomPanel {
    pub fn toggled(self) -> Self {
        match self {
            BottomPanel::Plots => BottomPanel::RawSentences,
            BottomPanel::RawSentences => BottomPanel::Plots,
        }
    }
}

/// `transport` decides which affordances the dashboard offers: a live receiver
/// gets liveness and acquisition readouts, a recording gets position and rate.
pub fn draw(frame: &mut Frame, state: &GnssState, bottom: BottomPanel, transport: &Transport) {
    let [header, main, bottom_area, footer] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(10),
        Constraint::Length(11),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    draw_header(frame, header, state, transport);

    let [track_area, side] =
        Layout::horizontal([Constraint::Min(30), Constraint::Length(40)]).areas(main);
    draw_track(frame, track_area, state);

    // The live panel carries one extra row (data rate and acquisition), so give
    // it the space only when it has something to say.
    let status_height = if matches!(transport, Transport::Live) {
        14
    } else {
        12
    };
    let [status_area, sats_area] =
        Layout::vertical([Constraint::Length(status_height), Constraint::Min(5)]).areas(side);
    draw_status(frame, status_area, state, transport);
    draw_satellites(frame, sats_area, state);

    match bottom {
        BottomPanel::Plots => draw_plots(frame, bottom_area, state),
        BottomPanel::RawSentences => draw_raw(frame, bottom_area, state),
    }

    draw_footer(frame, footer, transport);
}

fn fix_style(mode: FixMode) -> Style {
    match mode {
        FixMode::NoFix => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        FixMode::Fix2D => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        FixMode::Fix3D => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    }
}

fn constellation_color(constellation: Constellation) -> Color {
    match constellation {
        Constellation::Gps => Color::Cyan,
        Constellation::Glonass => Color::Magenta,
        Constellation::Galileo => Color::Green,
        Constellation::Beidou => Color::Yellow,
        Constellation::Qzss => Color::Blue,
        Constellation::NavIC => Color::LightRed,
    }
}

fn draw_header(frame: &mut Frame, area: Rect, state: &GnssState, transport: &Transport) {
    let counters = &state.counters;
    let source = if state.source_label.is_empty() {
        "no source".to_string()
    } else {
        state.source_label.clone()
    };

    // A recording that reached its end is not a fault, so it must not read as
    // one; only a live link going down is red.
    let controls = transport.controls();
    let connection_color = match state.connection.tone(controls) {
        StatusTone::Good => Color::Green,
        StatusTone::Warning => Color::Yellow,
        StatusTone::Bad => Color::Red,
        StatusTone::Neutral => Color::DarkGray,
    };

    let mut spans = vec![
        Span::styled(
            " WAYPOINT ",
            Style::default().fg(Color::Black).bg(Color::Cyan),
        ),
        Span::raw(format!(" {source}  ")),
        Span::styled(
            state.connection.label_for(controls),
            Style::default().fg(connection_color),
        ),
        Span::raw("   "),
        Span::styled(state.fix_mode.label(), fix_style(state.fix_mode)),
        Span::raw(format!(" / {}   ", state.fix_quality.label())),
        Span::raw(format!(
            "sentences {}  ok {}  crc-bad {}  err {}",
            counters.total, counters.decoded, counters.checksum_failed, counters.parse_failed
        )),
    ];

    match transport {
        // A recording has a position and a rate; it cannot go stale.
        Transport::Replay {
            speed,
            paused,
            progress,
        } => {
            if *paused {
                spans.push(Span::styled(
                    "   ‖ PAUSED",
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            spans.push(Span::styled(
                format!("   replay ×{speed}"),
                Style::default().fg(Color::Magenta),
            ));
            if let Some(fraction) = progress {
                spans.push(Span::styled(
                    format!("  {:.0}%", fraction * 100.0),
                    Style::default().fg(Color::Magenta),
                ));
            }
        }
        // A receiver has no position to report, but it can fall silent.
        Transport::Live => {
            let health = &state.health;
            if health.is_stale() {
                let since = health.since_last_sentence().unwrap_or_default();
                spans.push(Span::styled(
                    format!("   NO DATA {:.0}s", since.as_secs_f32()),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(
                    format!("   {:.1} msg/s", health.sentences_per_sec),
                    Style::default().fg(Color::Cyan),
                ));
            }
        }
        Transport::Instant | Transport::Idle => {}
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

/// Lat/lon plotted on a braille canvas — the terminal's stand-in for the GUI's
/// tile map. Bounds are corrected so the track keeps a roughly true shape
/// instead of being stretched by the cell aspect ratio.
fn draw_track(frame: &mut Frame, area: Rect, state: &GnssState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Track ({} points) ", state.track.len()));

    if state.track.is_empty() {
        frame.render_widget(
            Paragraph::new("\n  Waiting for a valid fix…")
                .style(Style::default().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }

    let (x_bounds, y_bounds) = track_bounds(state, area);
    let trail: Vec<(f64, f64)> = state.track.iter().map(|p| (p.lon, p.lat)).collect();
    let current = state.position().map(|(lat, lon)| (lon, lat));

    let canvas = Canvas::default()
        .block(block)
        .marker(Marker::Braille)
        .x_bounds(x_bounds)
        .y_bounds(y_bounds)
        .paint(move |ctx| {
            ctx.draw(&Points {
                coords: &trail,
                color: Color::Cyan,
            });
            if let Some(point) = current {
                ctx.layer();
                ctx.draw(&Points {
                    coords: std::slice::from_ref(&point),
                    color: Color::LightRed,
                });
            }
        });

    frame.render_widget(canvas, area);
}

/// Fit the track into the panel with a 10% margin, matching the panel's visual
/// aspect (terminal cells are about twice as tall as they are wide) and the
/// convergence of meridians at the current latitude.
fn track_bounds(state: &GnssState, area: Rect) -> ([f64; 2], [f64; 2]) {
    let (mut min_lat, mut max_lat) = (f64::MAX, f64::MIN);
    let (mut min_lon, mut max_lon) = (f64::MAX, f64::MIN);
    for point in &state.track {
        min_lat = min_lat.min(point.lat);
        max_lat = max_lat.max(point.lat);
        min_lon = min_lon.min(point.lon);
        max_lon = max_lon.max(point.lon);
    }

    let center_lat = (min_lat + max_lat) / 2.0;
    let center_lon = (min_lon + max_lon) / 2.0;
    let cos_lat = center_lat.to_radians().cos().abs().max(1e-6);

    // Visual width:height of the panel, in units where a cell is 1 wide by 2 tall.
    let panel_aspect = (area.width.saturating_sub(2)).max(1) as f64
        / ((area.height.saturating_sub(2)).max(1) as f64 * 2.0);

    let mut lat_span = (max_lat - min_lat).max(1e-5);
    // Longitude span expressed as the latitude span it covers on the ground.
    let mut lon_span_equiv = (max_lon - min_lon).max(1e-5) * cos_lat;

    if lon_span_equiv / lat_span > panel_aspect {
        lat_span = lon_span_equiv / panel_aspect;
    } else {
        lon_span_equiv = lat_span * panel_aspect;
    }

    let lat_pad = lat_span * 0.55;
    let lon_pad = lon_span_equiv / cos_lat * 0.55;

    (
        [center_lon - lon_pad, center_lon + lon_pad],
        [center_lat - lat_pad, center_lat + lat_pad],
    )
}

fn draw_status(frame: &mut Frame, area: Rect, state: &GnssState, transport: &Transport) {
    let by_constellation = state.satellites_by_constellation();
    let counts: Vec<Span> = by_constellation
        .iter()
        .map(|(constellation, sats)| {
            Span::styled(
                format!("{}{} ", constellation.short(), sats.len()),
                Style::default().fg(constellation_color(*constellation)),
            )
        })
        .collect();

    let used = state
        .sats_used
        .map(|n| n.to_string())
        .unwrap_or_else(|| "-".into());

    let mut lines = vec![
        value_line("Position", &format_position(state)),
        value_line(
            "Altitude",
            &state
                .altitude_m
                .map(|a| format!("{a:.1} m"))
                .unwrap_or_else(|| "-".into()),
        ),
        value_line(
            "Speed",
            &state
                .speed_knots
                .map(|s| format!("{s:.1} kt   {:.1} km/h", s * KNOTS_TO_KMH))
                .unwrap_or_else(|| "-".into()),
        ),
        value_line(
            "Course",
            &state
                .course_deg
                .map(|c| format!("{c:.1}°"))
                .unwrap_or_else(|| "-".into()),
        ),
        value_line(
            "Heading",
            &state
                .heading_deg
                .map(|h| format!("{h:.1}°"))
                .unwrap_or_else(|| "-".into()),
        ),
        value_line(
            "HDOP",
            &state
                .hdop
                .map(|h| format!("{h:.1} ({})", hdop_rating(h)))
                .unwrap_or_else(|| "-".into()),
        ),
        value_line(
            "PDOP/VDOP",
            &format!(
                "{} / {}",
                state.pdop.map(|v| format!("{v:.1}")).unwrap_or("-".into()),
                state.vdop.map(|v| format!("{v:.1}")).unwrap_or("-".into())
            ),
        ),
        value_line(
            "UTC",
            &state
                .fix_time
                .map(|t| t.format("%H:%M:%S").to_string())
                .unwrap_or_else(|| "-".into()),
        ),
    ];

    lines.push(value_line(
        "Sats",
        &format!("{} used / {} in view", used, state.sats_in_view()),
    ));

    // Acquisition only means something for a receiver that had to search for a
    // fix; replaying a log that already contains one says nothing about it.
    if matches!(transport, Transport::Live) {
        lines.push(value_line(
            "TTFF",
            &state
                .time_to_first_fix
                .map(|ttff| format!("{:.1} s", ttff.as_secs_f32()))
                .unwrap_or_else(|| "acquiring…".into()),
        ));
        lines.push(value_line(
            "Data rate",
            &format!("{:.1} sentences/s", state.health.sentences_per_sec),
        ));
    }

    // Per-constellation counts get their own line: colour-coded, and too wide to
    // share a row with the totals in a narrow panel.
    let mut counts_line = vec![Span::styled(
        format!("{:<10}", ""),
        Style::default().fg(Color::DarkGray),
    )];
    counts_line.extend(counts);
    lines.push(Line::from(counts_line));

    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Fix ")),
        area,
    );
}

fn value_line<'a>(label: &'a str, value: &str) -> Line<'a> {
    Line::from(vec![
        Span::styled(format!("{label:<10}"), Style::default().fg(Color::DarkGray)),
        Span::raw(value.to_string()),
    ])
}

fn format_position(state: &GnssState) -> String {
    match state.position() {
        Some((lat, lon)) => format!("{lat:.6}, {lon:.6}"),
        None => "-".into(),
    }
}

/// The terminal's substitute for the GUI's polar sky view: the same per-satellite
/// data (elevation, azimuth, SNR, used-in-fix) as a sortable grid.
fn draw_satellites(frame: &mut Frame, area: Rect, state: &GnssState) {
    let rows: Vec<Row> = state
        .satellites
        .iter()
        .map(|sat| {
            let snr = sat.snr.unwrap_or(0.0);
            let bar_width = ((snr / 50.0).clamp(0.0, 1.0) * 8.0).round() as usize;
            let bar = "█".repeat(bar_width);
            let snr_color = match snr as u32 {
                0 => Color::DarkGray,
                1..=24 => Color::Red,
                25..=34 => Color::Yellow,
                _ => Color::Green,
            };

            Row::new(vec![
                Cell::from(format!("{:>3}", sat.prn)),
                Cell::from(sat.constellation.short())
                    .style(Style::default().fg(constellation_color(sat.constellation))),
                Cell::from(
                    sat.elevation
                        .map(|e| format!("{e:>2.0}°"))
                        .unwrap_or_else(|| "  -".into()),
                ),
                Cell::from(
                    sat.azimuth
                        .map(|a| format!("{a:>3.0}°"))
                        .unwrap_or_else(|| "   -".into()),
                ),
                Cell::from(format!("{:>2.0} {bar}", snr)).style(Style::default().fg(snr_color)),
                Cell::from(if sat.used_in_fix { "✓" } else { " " })
                    .style(Style::default().fg(Color::Green)),
            ])
        })
        .collect();

    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Length(4),
            Constraint::Length(4),
            Constraint::Min(11),
            Constraint::Length(1),
        ],
    )
    .header(
        Row::new(vec!["PRN", "SY", " ELE", "  AZ", "SNR", "U"])
            .style(Style::default().fg(Color::DarkGray)),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Satellites in view "),
    );

    frame.render_widget(table, area);
}

fn draw_plots(frame: &mut Frame, area: Rect, state: &GnssState) {
    let [plots, trip] =
        Layout::horizontal([Constraint::Min(30), Constraint::Length(40)]).areas(area);

    let [speed, altitude, hdop] = Layout::vertical([
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
        Constraint::Ratio(1, 3),
    ])
    .areas(plots);

    let width = plots.width.saturating_sub(2) as usize;
    draw_sparkline(
        frame,
        speed,
        "Speed (kt)",
        Color::Cyan,
        sample_series(state, width, |s| s.speed_knots),
    );
    draw_sparkline(
        frame,
        altitude,
        "Altitude (m)",
        Color::Green,
        sample_series(state, width, |s| s.altitude_m),
    );
    draw_sparkline(
        frame,
        hdop,
        "HDOP",
        Color::Yellow,
        sample_series(state, width, |s| s.hdop),
    );

    draw_trip(frame, trip, state);
}

/// Take the most recent `width` samples of one metric, newest last.
fn sample_series(
    state: &GnssState,
    width: usize,
    pick: impl Fn(&waypoint_core::MetricSample) -> Option<f32>,
) -> Vec<f32> {
    let skip = state.history.len().saturating_sub(width.max(1));
    state.history.iter().skip(skip).filter_map(pick).collect()
}

/// Sparklines take unsigned bars, so values are normalised to the window's own
/// min/max — the title carries the actual range.
fn draw_sparkline(frame: &mut Frame, area: Rect, label: &str, color: Color, values: Vec<f32>) {
    let (min, max) = values
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));

    let title = if values.is_empty() {
        format!(" {label} ")
    } else {
        format!(
            " {label}  now {:.1}  [{min:.1} – {max:.1}] ",
            values[values.len() - 1]
        )
    };

    // A constant series has no range to normalise against; draw it mid-height so
    // "steady" reads differently from "no data".
    let span = max - min;
    let data: Vec<u64> = if span <= f32::EPSILON {
        vec![50; values.len()]
    } else {
        values
            .iter()
            .map(|v| (((v - min) / span) * 100.0).round() as u64)
            .collect()
    };

    frame.render_widget(
        Sparkline::default()
            .data(data)
            .max(100)
            .style(Style::default().fg(color))
            .block(Block::default().borders(Borders::ALL).title(title)),
        area,
    );
}

fn draw_trip(frame: &mut Frame, area: Rect, state: &GnssState) {
    let trip = &state.trip;
    let distance_km = trip.distance_m / 1000.0;

    let lines = vec![
        value_line("Distance", &format!("{distance_km:.3} km")),
        value_line("Elapsed", &format_duration(trip.elapsed_secs())),
        value_line("Moving", &format_duration(trip.moving_secs)),
        value_line("Stopped", &format_duration(trip.stopped_secs())),
        value_line(
            "Avg speed",
            &trip
                .avg_moving_speed_knots()
                .map(|s| format!("{s:.1} kt moving"))
                .unwrap_or_else(|| "-".into()),
        ),
        value_line("Max speed", &format!("{:.1} kt", trip.max_speed_knots)),
        value_line(
            "Elevation",
            &format!(
                "+{:.0} / -{:.0} m",
                trip.elevation_gain_m, trip.elevation_loss_m
            ),
        ),
        value_line(
            "Avg HDOP",
            &trip
                .avg_hdop()
                .map(|h| format!("{h:.2}"))
                .unwrap_or_else(|| "-".into()),
        ),
        value_line("Rejected", &format!("{} jump(s)", trip.rejected_jumps)),
    ];

    frame.render_widget(
        Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Trip ")),
        area,
    );
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

/// Raw sentence view — the reason this is a debugger and not just a tracker.
fn draw_raw(frame: &mut Frame, area: Rect, state: &GnssState) {
    let capacity = area.height.saturating_sub(2) as usize;
    let skip = state.raw.len().saturating_sub(capacity);

    let lines: Vec<Line> = state
        .raw
        .iter()
        .skip(skip)
        .map(|raw| {
            let (tag, color) = match &raw.outcome {
                SentenceOutcome::Decoded(kind) => (kind.clone(), Color::Green),
                SentenceOutcome::ChecksumFailed => ("CRC".into(), Color::Red),
                SentenceOutcome::Unsupported => ("SKIP".into(), Color::DarkGray),
                SentenceOutcome::ParseError(_) => ("ERR".into(), Color::LightRed),
            };
            Line::from(vec![
                Span::styled(format!("{tag:<5} "), Style::default().fg(color)),
                Span::raw(raw.text.clone()),
            ])
        })
        .collect();

    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Raw sentences "),
        ),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, transport: &Transport) {
    // Only advertise keys that do something for the source in hand.
    let keys = match transport {
        Transport::Replay { paused, .. } => {
            let play = if *paused {
                "space resume"
            } else {
                "space pause"
            };
            format!(
                " q quit   r reset trip   n toggle raw/plots   {play}   -/+ speed   1 real time   R restart "
            )
        }
        Transport::Instant => " q quit   r reset trip   n toggle raw/plots   R reload ".to_string(),
        Transport::Live | Transport::Idle => {
            " q quit   r reset trip   n toggle raw/plots ".to_string()
        }
    };
    frame.render_widget(
        Paragraph::new(keys).alignment(Alignment::Left).dark_gray(),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use waypoint_core::Aggregator;

    /// Build a state by replaying the bundled sample log through the aggregator.
    fn sample_state() -> GnssState {
        let log = include_str!("../../../assets/sample.nmea");
        let mut state = GnssState::new("file assets/sample.nmea".into());
        let mut aggregator = Aggregator::default();
        for line in log.lines() {
            aggregator.ingest_line(&mut state, line);
        }
        state
    }

    fn render(bottom: BottomPanel) -> String {
        render_transport(bottom, &Transport::Instant)
    }

    fn render_transport(bottom: BottomPanel, transport: &Transport) -> String {
        let state = sample_state();
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal
            .draw(|frame| draw(frame, &state, bottom, transport))
            .unwrap();
        buffer_text(&terminal)
    }

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn renders_dashboard_panels() {
        let out = render(BottomPanel::Plots);
        println!("{out}");

        for expected in [
            "WAYPOINT",
            "Track",
            "Fix",
            "Satellites in view",
            "Speed (kt)",
            "Trip",
            "Distance",
        ] {
            assert!(out.contains(expected), "missing {expected:?} in:\n{out}");
        }
        // The sample log ends with a 3D fix from a healthy multi-constellation receiver.
        assert!(out.contains("3D"));
    }

    /// Transport controls belong to a recording, never to a receiver.
    #[test]
    fn replay_shows_position_and_rate() {
        let out = render_transport(
            BottomPanel::Plots,
            &Transport::Replay {
                speed: 8.0,
                paused: false,
                progress: Some(0.42),
            },
        );

        assert!(out.contains("replay ×8"), "missing rate in:\n{out}");
        assert!(out.contains("42%"), "missing progress in:\n{out}");
        assert!(out.contains("space pause"));
        assert!(out.contains("R restart"));
        // Liveness readouts are meaningless for a recording.
        assert!(!out.contains("msg/s"));
        assert!(!out.contains("TTFF"));
    }

    #[test]
    fn paused_replay_is_called_out() {
        let out = render_transport(
            BottomPanel::Plots,
            &Transport::Replay {
                speed: 1.0,
                paused: true,
                progress: Some(0.5),
            },
        );

        assert!(out.contains("PAUSED"), "missing pause state in:\n{out}");
        assert!(out.contains("space resume"));
    }

    /// The connection metaphor does not belong to a file: it is opened and
    /// reaches an end, it does not connect or drop out.
    #[test]
    fn recording_status_avoids_connection_wording() {
        let mut state = sample_state();
        state.connection = waypoint_core::ConnectionStatus::Disconnected;

        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &state,
                    BottomPanel::Plots,
                    &Transport::Replay {
                        speed: 1.0,
                        paused: false,
                        progress: Some(1.0),
                    },
                )
            })
            .unwrap();
        let out = buffer_text(&terminal);

        assert!(out.contains("End of log"), "expected end-of-log in:\n{out}");
        assert!(!out.contains("Disconnected"));
    }

    /// A receiver gets liveness and acquisition instead, and none of the
    /// transport affordances it could not honour.
    #[test]
    fn live_source_shows_liveness_not_transport() {
        let out = render_transport(BottomPanel::Plots, &Transport::Live);

        assert!(out.contains("TTFF"), "missing acquisition row in:\n{out}");
        assert!(out.contains("Data rate"), "missing data rate in:\n{out}");
        assert!(!out.contains("replay ×"));
        assert!(!out.contains("space pause"));
        assert!(!out.contains("R restart"));
    }

    #[test]
    fn renders_raw_sentence_view() {
        let out = render(BottomPanel::RawSentences);
        println!("{out}");
        assert!(out.contains("Raw sentences"));
        assert!(out.contains("$GN"));
    }
}
