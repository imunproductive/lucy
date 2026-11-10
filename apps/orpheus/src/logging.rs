//! Console and file logging. `RUST_LOG` overrides the default `info` level.
//!
//! The file rotates like `ow174/log.py`: at 2 MB it becomes `orpheus.log.1`,
//! and two older files are kept.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

use tracing_subscriber::{EnvFilter, fmt, prelude::*};

pub const LOG_FILE: &str = "orpheus.log";
const LOG_FILE_BYTES: u64 = 2_000_000;
const LOG_FILE_BACKUPS: u32 = 2;

/// Log to the console, and when given a folder, also to its log file.
pub fn init(folder: Option<&Path>) -> io::Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let console = fmt::layer().with_writer(io::stderr).with_target(false);
    let file = match folder {
        Some(folder) => {
            fs::create_dir_all(folder)?;
            let file = RotatingFile::open(folder.join(LOG_FILE), LOG_FILE_BYTES, LOG_FILE_BACKUPS)?;
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

/// A log file that starts over when the next write would reach its size
/// limit (Python's `RotatingFileHandler`): `name.1` is the newest backup.
struct RotatingFile {
    path: PathBuf,
    max_bytes: u64,
    backups: u32,
    file: Option<File>,
    size: u64,
}

impl RotatingFile {
    fn open(path: PathBuf, max_bytes: u64, backups: u32) -> io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata()?.len();
        Ok(Self {
            path,
            max_bytes,
            backups,
            file: Some(file),
            size,
        })
    }

    fn backup(&self, number: u32) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(format!(".{number}"));
        name.into()
    }

    fn rotate(&mut self) -> io::Result<()> {
        // Windows renames only a closed file.
        self.file = None;
        for number in (1..self.backups).rev() {
            let from = self.backup(number);
            if from.exists() {
                replace(&from, &self.backup(number + 1))?;
            }
        }
        if self.backups > 0 {
            replace(&self.path, &self.backup(1))?;
        } else {
            fs::remove_file(&self.path)?;
        }
        self.file = Some(File::create(&self.path)?);
        self.size = 0;
        Ok(())
    }
}

fn replace(from: &Path, to: &Path) -> io::Result<()> {
    match fs::remove_file(to) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    fs::rename(from, to)
}

impl Write for RotatingFile {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.size > 0 && self.size + buffer.len() as u64 >= self.max_bytes {
            self.rotate()?;
        }
        let file = match &mut self.file {
            Some(file) => file,
            None => self.file.insert(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&self.path)?,
            ),
        };
        let written = file.write(buffer)?;
        self.size += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.as_mut().map_or(Ok(()), File::flush)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_rotates_at_its_size_and_keeps_two_backups() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(LOG_FILE);
        fs::write(&path, "old\n").unwrap();
        let mut file = RotatingFile::open(path.clone(), 10, 2).unwrap();
        for line in ["first\n", "second\n", "third\n", "fourth\n"] {
            file.write_all(line.as_bytes()).unwrap();
        }
        let read = |path: &Path| fs::read_to_string(path).unwrap();
        assert_eq!(read(&path), "fourth\n");
        assert_eq!(read(&file.backup(1)), "third\n");
        assert_eq!(read(&file.backup(2)), "second\n");
        assert!(!file.backup(3).exists());
    }
}
