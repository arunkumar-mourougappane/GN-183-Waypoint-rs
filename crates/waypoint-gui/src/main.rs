//! Map dashboard frontend for the Waypoint NMEA debugger.

mod app;
mod cli;
mod map;
mod panels;
mod skyview;

use anyhow::Result;
use clap::Parser;
use waypoint_core::available_serial_ports;

use crate::app::WaypointApp;
use crate::cli::Cli;

fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

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

    // eframe owns the main thread, so the engine's tasks run on a runtime we
    // keep alive alongside it.
    let runtime = tokio::runtime::Runtime::new()?;
    let initial_source = cli.source.to_config(&cli.options);
    let initial_recording = cli.options.record.clone();

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1400.0, 880.0])
            .with_min_inner_size([900.0, 600.0])
            .with_title("Waypoint — NMEA 0183 debugger"),
        ..Default::default()
    };

    eframe::run_native(
        "waypoint",
        options,
        Box::new(move |cc| {
            Ok(Box::new(WaypointApp::new(
                cc,
                runtime,
                initial_source,
                initial_recording,
            )))
        }),
    )
    .map_err(|err| anyhow::anyhow!("{err}"))
}
