//! Terminal frontend for the Waypoint NMEA debugger.

mod cli;
mod ui;

use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use waypoint_core::{Engine, TripConfig, available_serial_ports};

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
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Char('r') => engine.reset_trip(),
                    KeyCode::Char('n') => bottom = bottom.toggled(),
                    _ => {}
                }
            }
        }

        let state = engine.state();
        terminal.draw(|frame| ui::draw(frame, &state, bottom))?;
    }
}
