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

    /// Replay a log in its original timing instead of loading it at once
    #[arg(long)]
    pub replay: bool,

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
            let mode = if options.replay {
                FileMode::Replay {
                    control: ReplayControl::new(options.speed),
                }
            } else {
                FileMode::Instant
            };
            return Some(SourceConfig::File {
                path: path.clone(),
                mode,
            });
        }
        None
    }
}
