use std::{fmt, io, path::PathBuf};

#[derive(Debug)]
pub enum Error {
    Io {
        path: PathBuf,
        source: io::Error,
    },
    /// The install or one of its files does not have the expected layout.
    Format(String),
    /// A key, asset or file the storage does not hold.
    NotFound(String),
    /// An encrypted block whose key is not in the install's keyring.
    MissingKey(u64),
}

impl Error {
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Error {
        let path = path.into();
        move |source| Error::Io { path, source }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
            Error::Format(message) | Error::NotFound(message) => formatter.write_str(message),
            Error::MissingKey(name) => {
                write!(
                    formatter,
                    "encryption key {name:016X} is not in the install's keyring"
                )
            }
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

pub(crate) fn format(message: impl Into<String>) -> Error {
    Error::Format(message.into())
}
