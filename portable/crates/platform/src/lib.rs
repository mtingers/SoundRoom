//! Worker-only private storage capabilities. Open directories and files remain
//! the authority: descendants are never reached by reopening a checked path.
use std::{ffi::OsStr, fs::File, io, path::Path};
#[cfg(unix)]
mod unix;
#[cfg(windows)]
pub mod windows;
#[cfg(unix)]
use unix as backend;
#[cfg(windows)]
use windows as backend;

pub(crate) fn policy(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
/// A single child name, with identical safety semantics on every host. This
/// excludes paths, Windows streams/device names, drive syntax and dot aliases.
pub(crate) fn child_name(name: &OsStr) -> io::Result<()> {
    let text = name
        .to_str()
        .ok_or_else(|| policy("Private child name must be UTF-8"))?;
    let base = text.split('.').next().unwrap_or("").to_ascii_uppercase();
    if text.is_empty()
        || text.len() > 240
        || text == "."
        || text == ".."
        || text.ends_with(['.', ' '])
        || text
            .chars()
            .any(|c| c.is_control() || "/\\:<>\"|?*".contains(c))
        || [
            "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7",
            "COM8", "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
        ]
        .contains(&base.as_str())
    {
        return Err(policy("Invalid private child name"));
    }
    Ok(())
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileIdentity {
    pub volume: u64,
    pub object: u64,
}
pub struct PrivateDirectory {
    inner: backend::Directory,
}
impl PrivateDirectory {
    /// Open a configured absolute root with no symlink/reparse traversal. The
    /// final directory must already belong exclusively to the current identity.
    pub fn open_root(path: &Path) -> io::Result<Self> {
        Ok(Self {
            inner: backend::Directory::root(path, false)?,
        })
    }
    /// Exclusively create the final component of a configured absolute root.
    /// Existing roots reject; callers explicitly reopen after validating them.
    pub fn create_root(path: &Path) -> io::Result<Self> {
        Ok(Self {
            inner: backend::Directory::root(path, true)?,
        })
    }
    pub fn open_dir(&self, name: impl AsRef<OsStr>) -> io::Result<Self> {
        child_name(name.as_ref())?;
        Ok(Self {
            inner: self.inner.directory(name.as_ref(), false)?,
        })
    }
    pub fn create_dir(&self, name: impl AsRef<OsStr>) -> io::Result<Self> {
        child_name(name.as_ref())?;
        Ok(Self {
            inner: self.inner.directory(name.as_ref(), true)?,
        })
    }
    /// Read handles reject broad permissions/ACLs, hard links and nonregular
    /// nodes. Windows sharing prevents concurrent writers/deleters.
    pub fn read_file(&self, name: impl AsRef<OsStr>) -> io::Result<File> {
        child_name(name.as_ref())?;
        self.inner.file(name.as_ref(), false)
    }
    /// Exclusive read/write creation allows WAV header patching without reopen.
    pub fn create_file(&self, name: impl AsRef<OsStr>) -> io::Result<File> {
        child_name(name.as_ref())?;
        self.inner.file(name.as_ref(), true)
    }
    pub fn identity(&self) -> io::Result<FileIdentity> {
        self.inner.identity()
    }
    pub fn revalidate(&self) -> io::Result<()> {
        self.inner.revalidate()
    }
    pub fn sync(&self) -> io::Result<()> {
        self.inner.sync()
    }
    /// Same-parent exclusive publication. Retained source identity is verified
    /// before/after; existing targets, aliases, stale stages and other volumes
    /// reject. Files must have synced before this directory operation.
    pub fn publish(
        &self,
        source_name: impl AsRef<OsStr>,
        source: &Self,
        target_name: impl AsRef<OsStr>,
    ) -> io::Result<()> {
        child_name(source_name.as_ref())?;
        child_name(target_name.as_ref())?;
        self.inner
            .publish(source_name.as_ref(), &source.inner, target_name.as_ref())
    }
    /// Only regular single-link private files can be removed. Never follows a
    /// link, recursively removes a caller path or changes an existing broad ACL.
    pub fn remove_file(&self, name: impl AsRef<OsStr>) -> io::Result<()> {
        child_name(name.as_ref())?;
        self.inner.remove_file(name.as_ref())
    }
    /// Remove an empty held child after checking its identity. Cleanup callers
    /// enumerate their own created files; unknown content is retained.
    pub fn remove_dir(&self, name: impl AsRef<OsStr>, directory: &Self) -> io::Result<()> {
        child_name(name.as_ref())?;
        self.inner.remove_dir(name.as_ref(), &directory.inner)
    }
}
pub fn file_identity(file: &File) -> io::Result<FileIdentity> {
    backend::file_identity(file)
}
pub fn verify_private_file(file: &File) -> io::Result<()> {
    backend::verify_private_file(file)
}
