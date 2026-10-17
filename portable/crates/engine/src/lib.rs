//! Portable session authority and worker-side rendering. Hardware adapters are deliberately separate.
pub mod aiff;
pub mod authority;
pub mod automation;
pub mod automation_write;
pub mod capture;
pub mod capture_archive;
pub mod capture_clock;
pub mod capture_recovery;
pub mod capture_resample;
pub mod capture_worker;
#[cfg(target_os = "macos")]
pub mod core_audio;
#[cfg(target_os = "macos")]
pub mod core_midi;
pub mod custom_kits;
pub mod designer;
pub mod drums;
mod dsp;
pub mod envelope;
pub mod mastering;
pub mod measured_drums;
pub mod measured_instruments;
pub mod media;
#[cfg(target_os = "macos")]
pub mod media_decode;
pub mod meters;
pub mod midi;
pub mod model;
pub mod paged;
pub mod pitch;
pub mod pitch_targets;
pub mod preview;
pub mod project;
pub mod realtime;
pub mod render;
pub mod routing;
pub mod sampled_drums;
pub mod sampled_instruments;
pub mod spectral;
pub mod storage;
pub mod wave;
pub type Result<T> = std::result::Result<T, Error>;
#[derive(Debug)]
pub enum Error {
    Invalid(String),
    Unsupported(String),
    Conflict(String),
    Io(std::io::Error),
    Json(serde_json::Error),
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(x) | Self::Unsupported(x) | Self::Conflict(x) => f.write_str(x),
            Self::Io(x) => x.fmt(f),
            Self::Json(x) => x.fmt(f),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
