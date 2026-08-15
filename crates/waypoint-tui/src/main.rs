//! Terminal frontend for the Waypoint NMEA debugger.

mod cli;
mod ui;

use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use waypoint_core::{Engine, SourceControls, TripConfig, available_serial_ports};

use crate::cli::Cli;
use crate::ui::BottomPanel;

/// Input poll cadence. Also caps the redraw rate when data is arriving faster
/// than a terminal can usefully show.
const TICK: Duration = Duration::from_millis(50);

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Logs would corrupt the alternate screen, so they go to a file only when asked.
    if let Ok(path) = std::env::var("WAYPOINT_LOG") {
        let file = std::fs::File::create(path)?;
        tracing_subscriber::fmt()
            .with_writer(file)
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .init();
    }

    if cli.source.list_ports {
        let ports = available_serial_ports();
        if ports.is_empty() {
            println!("No serial ports found.");
        } else {
            for port in ports {
                println!("{port}");
            }
        }
        return Ok(());
    }

    let Some(config) = cli.source.to_config(&cli.options) else {
        anyhow::bail!("no source selected; pass --serial, --tcp or --file");
    };

    let engine = Engine::new(TripConfig::default());
    engine.set_source(config);

    let mut terminal = ratatui::init();
    let result = run(&engine, &mut terminal).await;
    ratatui::restore();
    result
}

async fn run(engine: &Engine, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    let mut updates = engine.updates();
    let mut bottom = BottomPanel::Plots;

    loop {
        // Redraw when new data lands, or on the tick so input stays responsive
        // on a silent stream.
        tokio::select! {
            _ = updates.changed() => {}
            _ = tokio::time::sleep(TICK) => {}
        }

        while event::poll(Duration::ZERO)? {
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                // Transport actions are simply inert for a live source: the
                // footer never advertises them, and replay_control is None.
                match Action::for_key(key.code) {
                    Action::Quit => return Ok(()),
                    Action::ResetTrip => engine.reset_trip(),
                    Action::ToggleBottomPanel => bottom = bottom.toggled(),
                    Action::TogglePause => {
                        if let Some(control) = engine.replay_control() {
                            control.toggle_paused();
                        }
                    }
                    Action::Restart => engine.restart(),
                    // Rate changes apply to the running replay — no restart, so
                    // the accumulated track survives.
                    Action::ScaleReplaySpeed(factor) => {
                        if let Some(control) = engine.replay_control() {
                            control.scale_speed(factor);
                        }
                    }
                    Action::SetReplaySpeed(rate) => {
                        if let Some(control) = engine.replay_control() {
                            control.set_speed(rate);
                        }
                    }
                    Action::Ignore => {}
                }
            }
        }

        let transport = Transport::sample(engine);
        let state = engine.state();
        terminal.draw(|frame| ui::draw(frame, &state, bottom, &transport))?;
    }
}

/// A frame's worth of transport state, sampled from the engine before drawing.
///
/// Rendering branches on this rather than on the source enum, so the dashboard
/// shows a receiver's liveness or a recording's position, never both.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Transport {
    /// Nothing running yet.
    Idle,
    /// A receiver in real time: no position, no rate control.
    Live,
    Replay {
        speed: f32,
        paused: bool,
        /// Fraction of the log consumed, when its length is known.
        progress: Option<f32>,
    },
    /// A log parsed in one go; over before there is anything to control.
    Instant,
}

impl Transport {
    fn sample(engine: &Engine) -> Self {
        match engine.controls() {
            None => Transport::Idle,
            Some(SourceControls::Live) => Transport::Live,
            Some(SourceControls::Instant) => Transport::Instant,
            Some(SourceControls::Replay) => match engine.replay_control() {
                Some(control) => Transport::Replay {
                    speed: control.speed(),
                    paused: control.is_paused(),
                    progress: control.progress(),
                },
                None => Transport::Idle,
            },
        }
    }
}

/// What a keypress means. Split out from the event loop so the bindings can be
/// tested without a terminal — the mapping is small, but it is exactly where a
/// control that silently does nothing would hide.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Action {
    Quit,
    ResetTrip,
    ToggleBottomPanel,
    /// Pause or resume a replay. Meaningless for a live receiver.
    TogglePause,
    /// Re-run a recording from the beginning.
    Restart,
    /// Multiply the replay rate, so stepping reads as "half"/"double".
    ScaleReplaySpeed(f32),
    SetReplaySpeed(f32),
    Ignore,
}

impl Action {
    fn for_key(code: KeyCode) -> Self {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
            KeyCode::Char('r') => Action::ResetTrip,
            KeyCode::Char('n') => Action::ToggleBottomPanel,
            KeyCode::Char(' ') => Action::TogglePause,
            KeyCode::Char('R') => Action::Restart,
            // '=' and '_' are the unshifted keys people actually hit for +/-.
            KeyCode::Char('+') | KeyCode::Char('=') => Action::ScaleReplaySpeed(2.0),
            KeyCode::Char('-') | KeyCode::Char('_') => Action::ScaleReplaySpeed(0.5),
            KeyCode::Char('1') => Action::SetReplaySpeed(1.0),
            _ => Action::Ignore,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waypoint_core::ReplayControl;

    #[test]
    fn replay_speed_keys_are_bound() {
        assert_eq!(
            Action::for_key(KeyCode::Char('+')),
            Action::ScaleReplaySpeed(2.0)
        );
        assert_eq!(
            Action::for_key(KeyCode::Char('=')),
            Action::ScaleReplaySpeed(2.0)
        );
        assert_eq!(
            Action::for_key(KeyCode::Char('-')),
            Action::ScaleReplaySpeed(0.5)
        );
        assert_eq!(
            Action::for_key(KeyCode::Char('_')),
            Action::ScaleReplaySpeed(0.5)
        );
        assert_eq!(
            Action::for_key(KeyCode::Char('1')),
            Action::SetReplaySpeed(1.0)
        );
    }

    #[test]
    fn control_keys_are_bound() {
        assert_eq!(Action::for_key(KeyCode::Char('q')), Action::Quit);
        assert_eq!(Action::for_key(KeyCode::Esc), Action::Quit);
        assert_eq!(Action::for_key(KeyCode::Char('r')), Action::ResetTrip);
        assert_eq!(
            Action::for_key(KeyCode::Char('n')),
            Action::ToggleBottomPanel
        );
        assert_eq!(Action::for_key(KeyCode::Char('z')), Action::Ignore);
    }

    /// Pressing '+' four times from real time must reach 16x, and '1' must come
    /// straight back — this is the sequence the rate controls exist for.
    #[test]
    fn stepping_produces_the_expected_rates() {
        let control = ReplayControl::new(1.0);

        for _ in 0..4 {
            match Action::for_key(KeyCode::Char('+')) {
                Action::ScaleReplaySpeed(factor) => control.scale_speed(factor),
                other => panic!("unexpected {other:?}"),
            }
        }
        assert_eq!(control.speed(), 16.0);

        match Action::for_key(KeyCode::Char('-')) {
            Action::ScaleReplaySpeed(factor) => control.scale_speed(factor),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(control.speed(), 8.0);

        match Action::for_key(KeyCode::Char('1')) {
            Action::SetReplaySpeed(rate) => control.set_speed(rate),
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(control.speed(), 1.0);
    }

    #[test]
    fn transport_keys_are_bound() {
        assert_eq!(Action::for_key(KeyCode::Char(' ')), Action::TogglePause);
        assert_eq!(Action::for_key(KeyCode::Char('R')), Action::Restart);
    }

    /// Transport state is derived from what the source can do, not from which
    /// transport it happens to be.
    #[test]
    fn pausing_toggles_through_the_shared_handle() {
        let control = ReplayControl::new(1.0);
        assert!(!control.is_paused());

        match Action::for_key(KeyCode::Char(' ')) {
            Action::TogglePause => control.toggle_paused(),
            other => panic!("unexpected {other:?}"),
        }
        assert!(control.is_paused());

        control.toggle_paused();
        assert!(!control.is_paused());
    }
}
