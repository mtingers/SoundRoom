//! Worker-side atomic directory publication. Replacement must be explicit.
use crate::{Error, Result};
use std::path::Path;

/// Existing nonempty packages cannot be replaced by ordinary rename. Exchange
/// their names atomically, leaving the old package at `source` for cleanup.
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub fn replace_directory(source: &Path, target: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    extern "C" {
        fn renamex_np(
            source: *const std::ffi::c_char,
            target: *const std::ffi::c_char,
            flags: u32,
        ) -> std::ffi::c_int;
    }
    #[cfg(target_os = "linux")]
    extern "C" {
        fn renameat2(
            oldfd: std::ffi::c_int,
            source: *const std::ffi::c_char,
            newfd: std::ffi::c_int,
            target: *const std::ffi::c_char,
            flags: std::ffi::c_uint,
        ) -> std::ffi::c_int;
    }
    let source = path_string(source)?;
    let target = path_string(target)?;
    // RENAME_SWAP (Darwin) / RENAME_EXCHANGE (Linux) are both 2. Unsupported
    // filesystems fail without unlinking either package; there is no fallback.
    #[cfg(target_os = "macos")]
    let result = unsafe { renamex_np(source.as_ptr(), target.as_ptr(), 2) };
    #[cfg(target_os = "linux")]
    let result = unsafe { renameat2(-100, source.as_ptr(), -100, target.as_ptr(), 2) };
    if result == 0 {
        Ok(())
    } else {
        Err(Error::Io(std::io::Error::last_os_error()))
    }
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn replace_directory(_: &Path, _: &Path) -> Result<()> {
    Err(Error::Unsupported(
        "Safe project replacement is unavailable on this platform. Save with a new name.".into(),
    ))
}
#[cfg(unix)]
fn path_string(path: &Path) -> Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| Error::Invalid("Path contains NUL".into()))
}
#[cfg(target_os = "macos")]
pub fn commit_new_directory(source: &Path, target: &Path) -> Result<()> {
    extern "C" {
        fn renamex_np(
            source: *const std::ffi::c_char,
            target: *const std::ffi::c_char,
            flags: u32,
        ) -> std::ffi::c_int;
    }
    let source = path_string(source)?;
    let target = path_string(target)?;
    // Both C strings remain alive and NUL-terminated throughout the system call.
    let result = unsafe { renamex_np(source.as_ptr(), target.as_ptr(), 4) };
    if result == 0 {
        Ok(())
    } else {
        Err(Error::Io(std::io::Error::last_os_error()))
    }
}
#[cfg(target_os = "linux")]
pub fn commit_new_directory(source: &Path, target: &Path) -> Result<()> {
    extern "C" {
        fn renameat2(
            oldfd: std::ffi::c_int,
            source: *const std::ffi::c_char,
            newfd: std::ffi::c_int,
            target: *const std::ffi::c_char,
            flags: std::ffi::c_uint,
        ) -> std::ffi::c_int;
    }
    let source = path_string(source)?;
    let target = path_string(target)?;
    // AT_FDCWD=-100 and RENAME_NOREPLACE=1. Unsupported filesystems return an error.
    let result = unsafe { renameat2(-100, source.as_ptr(), -100, target.as_ptr(), 1) };
    if result == 0 {
        Ok(())
    } else {
        Err(Error::Io(std::io::Error::last_os_error()))
    }
}
#[cfg(target_os = "windows")]
pub fn commit_new_directory(source: &Path, target: &Path) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(source: *const u16, target: *const u16, flags: u32) -> i32;
    }
    fn wide(path: &Path) -> Result<Vec<u16>> {
        let mut value: Vec<u16> = path.as_os_str().encode_wide().collect();
        if value.contains(&0) {
            return Err(Error::Invalid("Path contains NUL".into()));
        }
        value.push(0);
        Ok(value)
    }
    let source = wide(source)?;
    let target = wide(target)?;
    // Deliberately omit REPLACE_EXISTING and COPY_ALLOWED: the staged directory
    // must rename on the same volume and every preexisting destination rejects.
    // WRITE_THROUGH requests synchronous movement; Windows durability/long-path
    // qualification is still external, not established by this source review.
    let result = unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 8) };
    if result != 0 {
        Ok(())
    } else {
        Err(Error::Io(std::io::Error::last_os_error()))
    }
}
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub fn commit_new_directory(_: &Path, _: &Path) -> Result<()> {
    Err(Error::Unsupported(
        "No qualified exclusive directory commit on this platform".into(),
    ))
}
