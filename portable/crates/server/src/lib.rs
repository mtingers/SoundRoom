//! Portable control service. Device/render callbacks never read network state.
pub mod endpoint;
pub mod registry;
pub mod service;
pub use soundroom_client::Error;

mod synthesis;

pub mod audio;

pub mod assets;

mod readiness;

pub mod write_curve;

mod write_source;
