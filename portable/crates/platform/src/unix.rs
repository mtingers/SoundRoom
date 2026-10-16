use crate::{policy, FileIdentity};
use std::{
    ffi::{CString, OsStr},
    fs::File,
    io,
    os::unix::{
        ffi::OsStrExt,
        fs::MetadataExt,
        io::{AsRawFd, FromRawFd},
    },
    path::{Component, Path},
};

fn name(value: &OsStr) -> io::Result<CString> {
    CString::new(value.as_bytes()).map_err(|_| policy("NUL in storage name"))
}
fn open_at(parent: i32, value: &OsStr, flags: i32, mode: u32) -> io::Result<File> {
    let value = name(value)?;
    // NUL-terminated name and live directory handle; returned fd is uniquely
    // owned, noninheritable, and no final symlink is followed.
    let fd = unsafe {
        libc::openat(
            parent,
            value.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            mode as libc::c_uint,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn directory(parent: i32, value: &OsStr) -> io::Result<File> {
    open_at(parent, value, libc::O_RDONLY | libc::O_DIRECTORY, 0)
}
fn verify(file: &File, dir: bool, private: bool) -> io::Result<()> {
    let meta = file.metadata()?;
    if (dir && !meta.is_dir())
        || (!dir && (!meta.is_file() || meta.nlink() != 1))
        || (private && (meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0))
    {
        return Err(policy(
            "Private storage requires owner-only real directories or single-link regular files",
        ));
    }
    #[cfg(target_os = "macos")]
    if private {
        verify_no_extended_acl(file)?;
    }
    Ok(())
}
#[cfg(target_os = "macos")]
fn verify_no_extended_acl(file: &File) -> io::Result<()> {
    use std::{ffi::c_void, ptr};
    extern "C" {
        fn acl_get_fd_np(fd: i32, kind: i32) -> *mut c_void;
        fn acl_get_entry(acl: *mut c_void, entry: i32, out: *mut *mut c_void) -> i32;
        fn acl_valid(acl: *mut c_void) -> i32;
        fn acl_free(acl: *mut c_void) -> i32;
    }
    let acl = unsafe { acl_get_fd_np(file.as_raw_fd(), 0x100) };
    if acl.is_null() {
        let error = io::Error::last_os_error();
        // On a valid held descriptor Darwin uses ENOENT/ENOATTR for an absent
        // ACL extended attribute. Every other retrieval failure rejects.
        if [Some(libc::ENOENT), Some(libc::ENOATTR)].contains(&error.raw_os_error()) {
            return Ok(());
        }
        return Err(error);
    }
    let valid = unsafe { acl_valid(acl) };
    let mut entry = ptr::null_mut();
    let result = unsafe { acl_get_entry(acl, 0, &mut entry) };
    let error = io::Error::last_os_error();
    unsafe {
        acl_free(acl);
    }
    if valid != 0 {
        return Err(policy("Invalid private storage extended ACL"));
    }
    // Darwin's acl_get_entry returns0 for a found entry, -1/EINVAL at the
    // end. Empty, independently validated ACLs are the only admitted policy.
    if result == -1 && error.raw_os_error() == Some(libc::EINVAL) {
        return Ok(());
    }
    if result == 0 {
        return Err(policy(
            "Private storage rejects extended ACL grants or inherited entries",
        ));
    }
    Err(error)
}
pub fn file_identity(file: &File) -> io::Result<FileIdentity> {
    let meta = file.metadata()?;
    Ok(FileIdentity {
        volume: meta.dev(),
        object: meta.ino(),
    })
}
pub fn verify_private_file(file: &File) -> io::Result<()> {
    verify(file, false, true)
}
pub struct Directory {
    file: File,
}
impl Directory {
    pub fn root(path: &Path, create: bool) -> io::Result<Self> {
        if !path.is_absolute() {
            return Err(policy("Private root must be absolute"));
        }
        if path
            .as_os_str()
            .as_bytes()
            .split(|&b| b == b'/')
            .any(|c| c == b"." || c == b"..")
        {
            return Err(policy("Private root must not contain dot aliases"));
        }
        let mut components = path.components();
        if components.next() != Some(Component::RootDir) {
            return Err(policy("Invalid private root"));
        }
        let names: Vec<_> = components
            .map(|c| match c {
                Component::Normal(v) => Ok(v),
                _ => Err(policy("Private root must not contain dot aliases")),
            })
            .collect::<io::Result<_>>()?;
        if names.is_empty() || names.len() > 256 {
            return Err(policy("Invalid private root depth"));
        }
        let mut parent = directory(libc::AT_FDCWD, OsStr::new("/"))?;
        for component in &names[..names.len() - 1] {
            parent = directory(parent.as_raw_fd(), component)?;
            verify(&parent, true, false)?;
        }
        let final_name = names[names.len() - 1];
        crate::child_name(final_name)?;
        if create {
            let final_name = name(final_name)?;
            if unsafe { libc::mkdirat(parent.as_raw_fd(), final_name.as_ptr(), 0o700) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let file = directory(parent.as_raw_fd(), final_name)?;
        verify(&file, true, true)?;
        if create {
            parent.sync_all()?;
        }
        Ok(Self { file })
    }
    pub fn directory(&self, value: &OsStr, create: bool) -> io::Result<Self> {
        self.revalidate()?;
        if create {
            let value = name(value)?;
            if unsafe { libc::mkdirat(self.file.as_raw_fd(), value.as_ptr(), 0o700) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let file = directory(self.file.as_raw_fd(), value)?;
        verify(&file, true, true)?;
        if create {
            self.sync()?;
        }
        Ok(Self { file })
    }
    pub fn file(&self, value: &OsStr, create: bool) -> io::Result<File> {
        self.revalidate()?;
        let flags = if create {
            libc::O_RDWR | libc::O_CREAT | libc::O_EXCL
        } else {
            libc::O_RDONLY | libc::O_NONBLOCK
        };
        let file = open_at(self.file.as_raw_fd(), value, flags, 0o600)?;
        verify_private_file(&file)?;
        Ok(file)
    }
    pub fn identity(&self) -> io::Result<FileIdentity> {
        file_identity(&self.file)
    }
    pub fn revalidate(&self) -> io::Result<()> {
        verify(&self.file, true, true)
    }
    pub fn sync(&self) -> io::Result<()> {
        self.revalidate()?;
        self.file.sync_all()
    }
    pub fn publish(
        &self,
        source_name: &OsStr,
        source: &Self,
        target_name: &OsStr,
    ) -> io::Result<()> {
        self.revalidate()?;
        source.revalidate()?;
        let expected = source.identity()?;
        if self.directory(source_name, false)?.identity()? != expected {
            return Err(policy("Stale staging directory identity"));
        }
        let from = name(source_name)?;
        let to = name(target_name)?;
        #[cfg(target_os = "macos")]
        let result = unsafe {
            libc::renameatx_np(
                self.file.as_raw_fd(),
                from.as_ptr(),
                self.file.as_raw_fd(),
                to.as_ptr(),
                libc::RENAME_EXCL,
            )
        };
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::renameat2(
                self.file.as_raw_fd(),
                from.as_ptr(),
                self.file.as_raw_fd(),
                to.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Exclusive handle-rooted publication unavailable",
        ));
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if result != 0 {
            return Err(io::Error::last_os_error());
        }
        if self.directory(target_name, false)?.identity()? != expected {
            return Err(policy(
                "Published directory identity changed; retain for review",
            ));
        }
        self.sync()
    }
    pub fn remove_file(&self, value: &OsStr) -> io::Result<()> {
        let file = self.file(value, false)?;
        verify_private_file(&file)?;
        let value = name(value)?;
        if unsafe { libc::unlinkat(self.file.as_raw_fd(), value.as_ptr(), 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
    pub fn remove_dir(&self, value: &OsStr, source: &Self) -> io::Result<()> {
        if self.directory(value, false)?.identity()? != source.identity()? {
            return Err(policy("Stale directory cleanup identity"));
        }
        let value = name(value)?;
        if unsafe { libc::unlinkat(self.file.as_raw_fd(), value.as_ptr(), libc::AT_REMOVEDIR) } != 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
