//! Windows protected storage: handle-based owner/DACL checks, no reparse files,
//! non-inheritable handles, and an exclusive sharing lock. Never changes a broad
//! existing ACL to hide an unsafe credential source.
type Error = std::io::Error;
use std::{
    ffi::c_void,
    fs::File,
    io::{Read, Write},
    path::Path,
    ptr,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE, INVALID_HANDLE_VALUE},
    Security::{
        AclSizeInformation,
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            GetSecurityInfo, SE_FILE_OBJECT,
        },
        EqualSid, GetAce, GetAclInformation, GetSecurityDescriptorControl, GetTokenInformation,
        IsValidSid, TokenUser, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL_SIZE_INFORMATION,
        DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, SECURITY_ATTRIBUTES, TOKEN_QUERY,
        TOKEN_USER,
    },
    Storage::FileSystem::{
        CreateDirectoryW, CreateFileW, GetFileInformationByHandle, MoveFileExW,
        BY_HANDLE_FILE_INFORMATION, CREATE_NEW, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, MOVEFILE_REPLACE_EXISTING,
        MOVEFILE_WRITE_THROUGH, OPEN_ALWAYS, OPEN_EXISTING, READ_CONTROL,
    },
    System::{
        Memory::LocalFree,
        SystemServices::{ACCESS_ALLOWED_ACE_TYPE, GENERIC_READ, GENERIC_WRITE, SE_DACL_PROTECTED},
        Threading::{GetCurrentProcess, OpenProcessToken},
    },
};
fn policy(message: &str) -> Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message)
}
fn last() -> Error {
    std::io::Error::last_os_error()
}
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                LocalFree(self.0 as isize);
            }
        }
    }
}
struct User {
    bytes: Vec<usize>,
}
impl User {
    fn current() -> Result<Self, Error> {
        let mut token = 0;
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(last());
        }
        let token = Handle(token);
        let mut size = 0;
        unsafe {
            GetTokenInformation(token.0, TokenUser, ptr::null_mut(), 0, &mut size);
        }
        if size < std::mem::size_of::<TOKEN_USER>() as u32 || size > 65536 {
            return Err(policy("Invalid process identity size"));
        }
        // usize alignment is required for TOKEN_USER and its pointer-bearing SID member.
        let mut bytes = vec![0usize; (size as usize).div_ceil(std::mem::size_of::<usize>())];
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                bytes.as_mut_ptr().cast(),
                size,
                &mut size,
            )
        } == 0
        {
            return Err(last());
        }
        let out = Self { bytes };
        if unsafe { IsValidSid(out.sid()) } == 0 {
            return Err(policy("Invalid process identity"));
        }
        Ok(out)
    }
    fn sid(&self) -> *mut c_void {
        unsafe { (*self.bytes.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
    fn descriptor(&self, directory: bool) -> Result<Local, Error> {
        let mut text = ptr::null_mut();
        if unsafe { ConvertSidToStringSidW(self.sid(), &mut text) } == 0 {
            return Err(last());
        }
        let _text = Local(text.cast());
        let mut len = 0;
        unsafe {
            while *text.add(len) != 0 {
                len += 1;
                if len > 256 {
                    return Err(policy("Invalid process SID"));
                }
            }
        }
        let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(text, len) })
            .map_err(|_| policy("Invalid process SID"))?;
        // Protected owner-only full control; OICI lets private media/project descendants inherit it.
        let sddl = format!(
            "O:{sid}D:P(A;{};FA;;;{sid})",
            if directory { "OICI" } else { "" }
        );
        let sddl = wide_text(&sddl)?;
        let mut descriptor = ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(last());
        }
        Ok(Local(descriptor))
    }
}
fn wide_text(text: &str) -> Result<Vec<u16>, Error> {
    if text.contains('\0') {
        return Err(policy("NUL in storage path"));
    }
    Ok(text.encode_utf16().chain(Some(0)).collect())
}
#[cfg(windows)]
fn wide(path: &Path) -> Result<Vec<u16>, Error> {
    use std::os::windows::ffi::OsStrExt;
    let mut text = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if text.contains(&0) {
        return Err(policy("NUL in storage path"));
    }
    text.push(0);
    Ok(text)
}
#[cfg(windows)]
fn into_file(handle: Handle) -> File {
    use std::os::windows::io::FromRawHandle;
    let raw = handle.0;
    std::mem::forget(handle);
    unsafe { File::from_raw_handle(raw as *mut c_void) }
}
#[cfg(windows)]
fn raw(file: &File) -> HANDLE {
    use std::os::windows::io::AsRawHandle;
    file.as_raw_handle() as HANDLE
}
fn attributes(descriptor: &Local) -> SECURITY_ATTRIBUTES {
    SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: 0,
    }
}
fn open(
    path: &Path,
    access: u32,
    sharing: u32,
    disposition: u32,
    directory: bool,
    descriptor: Option<&Local>,
) -> Result<Handle, Error> {
    let text = wide(path)?;
    let attributes = descriptor.map(attributes);
    let handle = unsafe {
        CreateFileW(
            text.as_ptr(),
            access | READ_CONTROL,
            sharing,
            attributes.as_ref().map_or(ptr::null(), |a| a),
            disposition,
            FILE_FLAG_OPEN_REPARSE_POINT
                | if directory {
                    FILE_FLAG_BACKUP_SEMANTICS
                } else {
                    0
                },
            0,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(last());
    }
    Ok(Handle(handle))
}
fn verify(handle: HANDLE, user: &User, directory: bool) -> Result<u64, Error> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
        return Err(last());
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
        || (!directory && info.nNumberOfLinks != 1)
    {
        return Err(policy(
            "Private storage must be a real directory or single-link regular file",
        ));
    }
    let mut owner = ptr::null_mut();
    let mut dacl = ptr::null_mut();
    let mut descriptor = ptr::null_mut();
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            &mut dacl,
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != 0 {
        return Err(std::io::Error::from_raw_os_error(status as i32));
    }
    let _descriptor = Local(descriptor);
    let mut control = 0;
    let mut revision = 0;
    if owner.is_null()
        || dacl.is_null()
        || unsafe { IsValidSid(owner) } == 0
        || unsafe { EqualSid(owner, user.sid()) } == 0
        || unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
        || control as u32 & SE_DACL_PROTECTED == 0
    {
        return Err(policy(
            "Private storage must have current-user ownership and a protected DACL",
        ));
    }
    let mut info_acl: ACL_SIZE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe {
        GetAclInformation(
            dacl,
            (&mut info_acl as *mut ACL_SIZE_INFORMATION).cast(),
            std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
            AclSizeInformation,
        )
    } == 0
    {
        return Err(last());
    }
    // Deliberately strict: one explicit allow ACE for the current owner only.
    // Broad, inherited, conditional, object or deny ACEs are never silently accepted.
    if info_acl.AceCount != 1 {
        return Err(policy("Private storage requires one owner-only allow ACE"));
    }
    let mut ace = ptr::null_mut();
    if unsafe { GetAce(dacl, 0, &mut ace) } == 0 {
        return Err(last());
    }
    let header = unsafe { &*ace.cast::<ACE_HEADER>() };
    let permitted = if directory { 3 } else { 0 };
    if header.AceType as u32 != ACCESS_ALLOWED_ACE_TYPE
        || header.AceFlags & !permitted != 0
        || header.AceSize < (std::mem::size_of::<ACCESS_ALLOWED_ACE>() + 4) as u16
    {
        return Err(policy("Private storage has an unsupported owner ACE"));
    }
    let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
    if allowed.Mask == 0 {
        return Err(policy("Private storage owner ACE has no rights"));
    }
    let sid = ptr::addr_of!(allowed.SidStart) as *mut c_void;
    // Check the variable SID size before passing it to Win32 SID inspection.
    let sid_bytes = sid.cast::<u8>();
    let subauthorities = unsafe { *sid_bytes.add(1) } as usize;
    if unsafe { *sid_bytes } != 1
        || subauthorities > 15
        || (header.AceSize as usize) < 8 + 8 + 4 * subauthorities
    {
        return Err(policy("Private storage has a malformed owner SID"));
    }
    if unsafe { IsValidSid(sid) } == 0 || unsafe { EqualSid(sid, user.sid()) } == 0 {
        return Err(policy("Private storage grants another identity access"));
    }
    Ok((info.nFileSizeHigh as u64) << 32 | info.nFileSizeLow as u64)
}
pub fn private_directory(path: &Path) -> Result<(), Error> {
    let user = User::current()?;
    let descriptor = user.descriptor(true)?;
    let text = wide(path)?;
    let attributes = attributes(&descriptor);
    if unsafe { CreateDirectoryW(text.as_ptr(), &attributes) } == 0
        && std::io::Error::last_os_error().raw_os_error() != Some(ERROR_ALREADY_EXISTS as i32)
    {
        return Err(last());
    }
    let handle = open(
        path,
        0,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        OPEN_EXISTING,
        true,
        None,
    )?;
    verify(handle.0, &user, true)?;
    Ok(())
}
pub fn read_private(path: &Path, max: u64) -> Result<Vec<u8>, Error> {
    let user = User::current()?;
    let handle = open(
        path,
        GENERIC_READ,
        FILE_SHARE_READ,
        OPEN_EXISTING,
        false,
        None,
    )?;
    let size = verify(handle.0, &user, false)?;
    if size > max {
        return Err(policy("Private file exceeds its budget"));
    }
    let mut bytes = Vec::with_capacity(size.min(1_048_576) as usize);
    into_file(handle)
        .take(max.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(policy("Private file exceeds its budget"));
    }
    Ok(bytes)
}
pub fn write_new_private(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let user = User::current()?;
    let descriptor = user.descriptor(false)?;
    let handle = open(path, GENERIC_WRITE, 0, CREATE_NEW, false, Some(&descriptor))?;
    if let Err(error) = verify(handle.0, &user, false) {
        drop(handle);
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    let mut file = into_file(handle);
    let result = file.write_all(bytes).and_then(|_| file.sync_all());
    drop(file);
    if let Err(e) = result {
        let _ = std::fs::remove_file(path);
        return Err(e);
    }
    Ok(())
}
pub fn exclusive_lock(path: &Path) -> Result<File, Error> {
    let user = User::current()?;
    let descriptor = user.descriptor(false)?;
    let handle = open(
        path,
        GENERIC_READ | GENERIC_WRITE,
        0,
        OPEN_ALWAYS,
        false,
        Some(&descriptor),
    )?;
    verify(handle.0, &user, false)?;
    Ok(into_file(handle))
}
/// Both paths must be sibling private files in the owner-only storage directory.
pub fn replace_private(stage: &Path, destination: &Path) -> Result<(), Error> {
    if stage.parent() != destination.parent() {
        return Err(policy("Private replacement must stay in one directory"));
    }
    private_directory(
        stage
            .parent()
            .ok_or_else(|| policy("Missing storage directory"))?,
    )?;
    let user = User::current()?;
    {
        let file = open(
            stage,
            GENERIC_READ,
            FILE_SHARE_READ,
            OPEN_EXISTING,
            false,
            None,
        )?;
        verify(file.0, &user, false)?;
    }
    {
        let file = open(
            destination,
            GENERIC_READ,
            FILE_SHARE_READ,
            OPEN_EXISTING,
            false,
            None,
        )?;
        verify(file.0, &user, false)?;
    }
    let stage = wide(stage)?;
    let destination = wide(destination)?;
    if unsafe {
        MoveFileExW(
            stage.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    } == 0
    {
        return Err(last());
    }
    Ok(())
}
/// Explicit handle revalidation for host diagnostics; no paths or secrets enter logs.
pub fn verify_private_file(file: &File) -> Result<(), Error> {
    verify(raw(file), &User::current()?, false)?;
    Ok(())
}

// RootDirectory-based opens keep every child lookup attached to the actual
// retained directory, including through rename/reparse races. The layouts below
// are the user-mode winternl.h structures, not driver/kernel-only handles.
#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}
#[repr(C)]
struct ObjectAttributes {
    length: u32,
    root_directory: HANDLE,
    object_name: *mut UnicodeString,
    attributes: u32,
    security_descriptor: *mut c_void,
    security_quality: *mut c_void,
}
#[repr(C)]
struct IoStatusBlock {
    status: isize,
    information: usize,
}
const _: [(); 2 * std::mem::size_of::<usize>()] = [(); std::mem::size_of::<UnicodeString>()];
const _: [(); 6 * std::mem::size_of::<usize>()] = [(); std::mem::size_of::<ObjectAttributes>()];
const _: [(); 2 * std::mem::size_of::<usize>()] = [(); std::mem::size_of::<IoStatusBlock>()];
#[link(name = "ntdll")]
extern "system" {
    fn NtCreateFile(
        handle: *mut HANDLE,
        access: u32,
        object: *const ObjectAttributes,
        status: *mut IoStatusBlock,
        allocation: *const i64,
        attributes: u32,
        sharing: u32,
        disposition: u32,
        options: u32,
        ea: *const c_void,
        ea_length: u32,
    ) -> i32;
    fn RtlNtStatusToDosError(status: i32) -> u32;
}
const DELETE_ACCESS: u32 = 0x00010000;
const SYNCHRONIZE_ACCESS: u32 = 0x00100000;
fn relative(
    parent: HANDLE,
    value: &std::ffi::OsStr,
    directory: bool,
    create: bool,
    intermediate: bool,
) -> Result<Handle, Error> {
    relative_access(parent, value, directory, create, intermediate, false)
}
fn relative_access(
    parent: HANDLE,
    value: &std::ffi::OsStr,
    directory: bool,
    create: bool,
    intermediate: bool,
    delete: bool,
) -> Result<Handle, Error> {
    use std::os::windows::ffi::OsStrExt;
    let mut text = value.encode_wide().collect::<Vec<_>>();
    if text.is_empty() || text.len() > 32767 || text.contains(&0) {
        return Err(policy("Invalid relative storage name"));
    }
    let bytes = (text.len() * 2) as u16;
    let mut name = UnicodeString {
        length: bytes,
        maximum_length: bytes,
        buffer: text.as_mut_ptr(),
    };
    let user = User::current()?;
    let descriptor = if create {
        Some(user.descriptor(directory)?)
    } else {
        None
    };
    let object = ObjectAttributes {
        length: std::mem::size_of::<ObjectAttributes>() as u32,
        root_directory: parent,
        object_name: &mut name,
        attributes: 0x40,
        security_descriptor: descriptor.as_ref().map_or(ptr::null_mut(), |v| v.0),
        security_quality: ptr::null_mut(),
    };
    let mut status = IoStatusBlock {
        status: 0,
        information: 0,
    };
    let mut handle = 0;
    let access = READ_CONTROL
        | SYNCHRONIZE_ACCESS
        | if directory {
            // LIST_DIRECTORY/TRAVERSE/READ_ATTRIBUTES; DELETE on private children
            // enables publication by the retained source handle, never its old path.
            1 | 32 | 128 | if intermediate { 0 } else { DELETE_ACCESS }
        } else {
            GENERIC_READ
                | if create { GENERIC_WRITE } else { 0 }
                | if delete { DELETE_ACCESS } else { 0 }
        };
    let code = unsafe {
        NtCreateFile(
            &mut handle,
            access,
            &object,
            &mut status,
            ptr::null(),
            if directory {
                FILE_ATTRIBUTE_DIRECTORY
            } else {
                0x80
            },
            if directory {
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
            } else {
                FILE_SHARE_READ
            },
            if create { 2 } else { 1 },
            0x00200000 | 0x20 | if directory { 1 } else { 0x40 },
            ptr::null(),
            0,
        )
    };
    if code < 0 {
        return Err(std::io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(code) } as i32,
        ));
    }
    let handle = Handle(handle);
    if handle.0 == 0 || handle.0 == INVALID_HANDLE_VALUE {
        return Err(policy("Native storage open returned no valid handle"));
    }
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(handle.0, &mut info) } == 0 {
        return Err(last());
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
    {
        return Err(policy(
            "Private storage rejects reparse/incorrect-type nodes",
        ));
    }
    if !intermediate {
        verify(handle.0, &user, directory)?;
    }
    Ok(handle)
}
pub fn file_identity(file: &File) -> Result<crate::FileIdentity, Error> {
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(raw(file), &mut info) } == 0 {
        return Err(last());
    }
    Ok(crate::FileIdentity {
        volume: info.dwVolumeSerialNumber as u64,
        object: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
    })
}
pub struct Directory {
    file: File,
}
impl Directory {
    pub fn root(path: &Path, create: bool) -> Result<Self, Error> {
        use std::os::windows::ffi::OsStrExt;
        use std::path::{Component, Prefix};
        let encoded = path.as_os_str().encode_wide().collect::<Vec<_>>();
        if encoded
            .split(|&c| c == b'/' as u16 || c == b'\\' as u16)
            .any(|c| c == [b'.' as u16] || c == [b'.' as u16, b'.' as u16])
        {
            return Err(policy("Private root rejects dot aliases"));
        }
        let mut components = path.components();
        let disk = match components.next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::Disk(d) | Prefix::VerbatimDisk(d) => d,
                _ => {
                    return Err(policy(
                        "Private storage requires an absolute local drive path",
                    ))
                }
            },
            _ => {
                return Err(policy(
                    "Private storage requires an absolute local drive path",
                ))
            }
        };
        if components.next() != Some(Component::RootDir) {
            return Err(policy("Drive-relative storage rejects"));
        }
        let names: Vec<_> = components
            .map(|c| match c {
                Component::Normal(v) => {
                    crate::child_name(v)?;
                    Ok(v)
                }
                _ => Err(policy("Private root rejects dot aliases")),
            })
            .collect::<Result<_, Error>>()?;
        if names.is_empty() || names.len() > 256 {
            return Err(policy("Invalid storage root depth"));
        }
        let mut parent = open(
            Path::new(&format!("{}:\\", disk as char)),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            OPEN_EXISTING,
            true,
            None,
        )?;
        for value in &names[..names.len() - 1] {
            parent = relative(parent.0, value, true, false, true)?;
        }
        let file = into_file(relative(
            parent.0,
            names[names.len() - 1],
            true,
            create,
            false,
        )?);
        Ok(Self { file })
    }
    pub fn directory(&self, value: &std::ffi::OsStr, create: bool) -> Result<Self, Error> {
        self.revalidate()?;
        Ok(Self {
            file: into_file(relative(raw(&self.file), value, true, create, false)?),
        })
    }
    pub fn file(&self, value: &std::ffi::OsStr, create: bool) -> Result<File, Error> {
        self.revalidate()?;
        Ok(into_file(relative(
            raw(&self.file),
            value,
            false,
            create,
            false,
        )?))
    }
    pub fn identity(&self) -> Result<crate::FileIdentity, Error> {
        file_identity(&self.file)
    }
    pub fn revalidate(&self) -> Result<(), Error> {
        verify(raw(&self.file), &User::current()?, true)?;
        Ok(())
    }
    pub fn sync(&self) -> Result<(), Error> {
        // Windows does not provide a universally reliable directory FlushFileBuffers
        // contract. Child files are flushed before atomic handle rename. Actual
        // crash-durability qualification remains an external filesystem gate.
        self.revalidate()
    }
    pub fn publish(
        &self,
        source_name: &std::ffi::OsStr,
        source: &Self,
        target_name: &std::ffi::OsStr,
    ) -> Result<(), Error> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FileRenameInfo, SetFileInformationByHandle, FILE_RENAME_INFO,
        };
        self.revalidate()?;
        source.revalidate()?;
        let expected = source.identity()?;
        if self.directory(source_name, false)?.identity()? != expected {
            return Err(policy("Stale staging directory identity"));
        }
        let text = target_name.encode_wide().collect::<Vec<_>>();
        let byte_count = text.len() * 2;
        // Keep FILE_RENAME_INFO's pointer-sized fields aligned. repr(C) and its
        // one-element trailing array determine the offset on both x86/x64.
        let info: FILE_RENAME_INFO = unsafe { std::mem::zeroed() };
        let offset = ptr::addr_of!(info.FileName) as usize - ptr::addr_of!(info) as usize;
        let size = (offset + byte_count).max(std::mem::size_of::<FILE_RENAME_INFO>());
        let mut buffer = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
        let header = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        unsafe {
            (*header).RootDirectory = raw(&self.file);
            (*header).FileNameLength = byte_count as u32;
            ptr::copy_nonoverlapping(
                text.as_ptr(),
                buffer.as_mut_ptr().cast::<u8>().add(offset).cast::<u16>(),
                text.len(),
            );
        }
        // ReplaceIfExists is false (zero buffer). The source is the exact held
        // directory, and destination resolution is rooted in the held parent.
        if unsafe {
            SetFileInformationByHandle(
                raw(&source.file),
                FileRenameInfo,
                buffer.as_ptr().cast(),
                size as u32,
            )
        } == 0
        {
            return Err(last());
        }
        if self.directory(target_name, false)?.identity()? != expected {
            return Err(policy(
                "Published directory identity changed; retain for review",
            ));
        }
        self.sync()
    }
    pub fn remove_file(&self, value: &std::ffi::OsStr) -> Result<(), Error> {
        self.revalidate()?;
        let handle = relative_access(raw(&self.file), value, false, false, false, true)?;
        delete_handle(handle.0)
    }
    pub fn remove_dir(&self, value: &std::ffi::OsStr, source: &Self) -> Result<(), Error> {
        self.revalidate()?;
        source.revalidate()?;
        if self.directory(value, false)?.identity()? != source.identity()? {
            return Err(policy("Stale directory cleanup identity"));
        }
        delete_handle(raw(&source.file))
    }
}
fn delete_handle(handle: HANDLE) -> Result<(), Error> {
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
    };
    let info = FILE_DISPOSITION_INFO { DeleteFileA: 1 };
    if unsafe {
        SetFileInformationByHandle(
            handle,
            FileDispositionInfo,
            (&info as *const FILE_DISPOSITION_INFO).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        return Err(last());
    }
    Ok(())
}
