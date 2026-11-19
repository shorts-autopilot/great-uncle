use std::io;

/// Anything that can go wrong on an output path.
#[derive(Debug, thiserror::Error)]
pub enum DmxError {
    #[error("{}: {source}", path)]
    Io {
        path: String,
        #[source]
        source: io::Error,
    },
    /// The requested transport does not exist in this build or on this platform.
    #[error("{0}")]
    Unsupported(String),
    /// The device rejected or could not handle the frame.
    #[error("protocol error: {0}")]
    Protocol(String),
    /// USB control transfer failure.
    #[error("usb error: {0}")]
    Usb(String),
}

impl DmxError {
    pub fn io(path: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

pub type DmxResult<T> = Result<T, DmxError>;
