//! Typed control client. Audio and authored project state remain on the server.
pub mod auth;
pub mod client;
#[path = "../../../../protocol/generated/contract.rs"]
pub mod contract;
mod decode;
#[path = "../../../../protocol/generated/models.rs"]
pub mod models;
pub mod snapshot;
pub mod state;
pub mod transfer;
pub mod transport;
pub mod validation;
pub mod websocket;
mod write_validation;
use std::fmt;

#[derive(Debug)]
pub enum Error {
    Protocol(String),
    Io(std::io::Error),
    Tls(native_tls::Error),
    Json(serde_json::Error),
    Disconnected,
    Timeout,
    QueueFull,
    Conflict(String),
    Uncertain(String),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Protocol(message) | Self::Conflict(message) => write!(f, "{message}"),
            Self::Io(error) => error.fmt(f),
            Self::Tls(error) => error.fmt(f),
            Self::Json(error) => error.fmt(f),
            Self::Disconnected => write!(f, "Connection lost"),
            Self::Timeout => write!(f, "Connection timed out"),
            Self::QueueFull => write!(f, "Control queue is full"),
            Self::Uncertain(id) => write!(
                f,
                "Receipt for request {id} must be reconciled before retrying"
            ),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl From<native_tls::Error> for Error {
    fn from(error: native_tls::Error) -> Self {
        Self::Tls(error)
    }
}
impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

#[cfg(windows)]
pub mod windows_storage;
