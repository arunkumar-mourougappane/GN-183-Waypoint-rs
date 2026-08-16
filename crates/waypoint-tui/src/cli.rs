//! Source selection shared in shape with the GUI's flags, so a session can be
//! started the same way against either frontend.

use std::path::PathBuf;

use clap::{Args, Parser};
use waypoint_core::{FileMode, ReplayControl, SourceConfig};

#[derive(Debug, Parser)]
#[command(name = "waypoint-tui", version, about = "Terminal NMEA 0183 debugger")]
pub struct Cli {
    #[command(flatten)]
    pub source: SourceArgs,

    #[command(flatten)]
    pub options: SourceOptions,
}

#[derive(Debug, Args)]
#[group(required = true, multiple = false)]
pub struct SourceArgs {
    /// Serial port to read from, e.g. /dev/ttyUSB0 or COM3
    #[arg(long, value_name = "PORT")]
    pub serial: Option<String>,

    /// TCP endpoint to read from, e.g. 192.168.1.50:10110
    #[arg(long, value_name = "HOST:PORT")]
    pub tcp: Option<String>,

    /// NMEA log file to read
    #[arg(long, value_name = "PATH")]
    pub file: Option<PathBuf>,

    /// List available serial ports and exit
    #[arg(long)]
    pub list_ports: bool,
}

#[derive(Debug, Args)]
pub struct SourceOptions {
    /// Serial baud rate
    #[arg(long, default_value_t = 9600)]
    pub baud: u32,

    /// Parse a log as fast as it can be read, instead of replaying it in its
    /// original timing. Skips straight to the finished track.
    #[arg(long)]
    pub instant: bool,

    /// Replay speed multiplier
    #[arg(long, default_value_t = 1.0, value_name = "X")]
    pub speed: f32,
}

impl SourceArgs {
    /// Build a [`SourceConfig`], or `None` when only `--list-ports` was asked for.
    pub fn to_config(&self, options: &SourceOptions) -> Option<SourceConfig> {
        if let Some(path) = &self.serial {
            return Some(SourceConfig::Serial {
                path: path.clone(),
                baud: options.baud,
            });
        }
        if let Some(addr) = &self.tcp {
            return Some(SourceConfig::Tcp { addr: addr.clone() });
        }
        if let Some(path) = &self.file {
            // A log replays by default. Loading it instantly leaves nothing to
            // pause, retime or scrub, which looks like a frontend missing its
            // features rather than a deliberate mode.
            let mode = if options.instant {
                FileMode::Instant
            } else {
                FileMode::Replay {
                    control: ReplayControl::new(options.speed),
                }
            };
            return Some(SourceConfig::File {
                path: path.clone(),
                mode,
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use waypoint_core::SourceControls;

    fn config_for(args: &[&str]) -> SourceConfig {
        let cli = Cli::parse_from(args);
        cli.source
            .to_config(&cli.options)
            .expect("a source should have been built")
    }

    /// Opening a log must replay it. Loading it instantly leaves nothing to
    /// pause, retime or scrub, which reads as a frontend missing its features
    /// rather than as a deliberate mode.
    #[test]
    fn a_log_replays_by_default() {
        let config = config_for(&["waypoint", "--file", "track.nmea"]);
        assert_eq!(config.controls(), SourceControls::Replay);
    }

    #[test]
    fn instant_is_opt_in() {
        let config = config_for(&["waypoint", "--file", "track.nmea", "--instant"]);
        assert_eq!(config.controls(), SourceControls::Instant);
    }

    #[test]
    fn live_sources_are_never_replays() {
        assert_eq!(
            config_for(&["waypoint", "--serial", "/dev/ttyUSB0"]).controls(),
            SourceControls::Live
        );
        assert_eq!(
            config_for(&["waypoint", "--tcp", "127.0.0.1:10110"]).controls(),
            SourceControls::Live
        );
    }

    #[test]
    fn replay_speed_comes_from_the_flag() {
        let config = config_for(&["waypoint", "--file", "track.nmea", "--speed", "8"]);
        match config {
            SourceConfig::File {
                mode: FileMode::Replay { control },
                ..
            } => assert_eq!(control.speed(), 8.0),
            other => panic!("expected a replay, got {other:?}"),
        }
    }
}
