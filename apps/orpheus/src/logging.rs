//! Console and file logging. `RUST_LOG` overrides the default `info` level.

use std::{
    fs::{self, OpenOptions},
    io,
    path::Path,
    sync::Mutex,
};

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

pub const LOG_FILE: &str = "orpheus.log";

/// Log to the console, and when given a folder, also append to its log file.
pub fn init(folder: Option<&Path>) -> io::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let console = fmt::layer().with_writer(io::stderr).with_target(false);
    let file = match folder {
        Some(folder) => {
            fs::create_dir_all(folder)?;
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(folder.join(LOG_FILE))?;
            Some(fmt::layer().with_ansi(false).with_writer(Mutex::new(file)))
        }
        None => None,
    };
    tracing_subscriber::registry()
        .with(filter)
        .with(console)
        .with(file)
        .init();
    Ok(())
}
