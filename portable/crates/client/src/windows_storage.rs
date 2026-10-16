//! Compatibility facade preserving client policy error classification.
use crate::Error;
use std::{fs::File, path::Path};
fn convert(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::InvalidData {
        Error::Protocol(error.to_string())
    } else {
        error.into()
    }
}
pub fn private_directory(path: &Path) -> Result<(), Error> {
    soundroom_platform::windows::private_directory(path).map_err(convert)
}
pub fn read_private(path: &Path, max: u64) -> Result<Vec<u8>, Error> {
    soundroom_platform::windows::read_private(path, max).map_err(convert)
}
pub fn write_new_private(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    soundroom_platform::windows::write_new_private(path, bytes).map_err(convert)
}
pub fn exclusive_lock(path: &Path) -> Result<File, Error> {
    soundroom_platform::windows::exclusive_lock(path).map_err(convert)
}
pub fn replace_private(stage: &Path, target: &Path) -> Result<(), Error> {
    soundroom_platform::windows::replace_private(stage, target).map_err(convert)
}
pub fn verify_private_file(file: &File) -> Result<(), Error> {
    soundroom_platform::windows::verify_private_file(file).map_err(convert)
}
