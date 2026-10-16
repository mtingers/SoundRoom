use soundroom_platform::{file_identity, verify_private_file, PrivateDirectory};
use std::{
    fs,
    io::{Read, Write},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static ID: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    path: PathBuf,
    directory: PrivateDirectory,
}
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "soundroom-custody-{}-{}",
            std::process::id(),
            ID.fetch_add(1, Ordering::Relaxed)
        ));
        let directory = PrivateDirectory::create_root(&path).unwrap();
        Self { path, directory }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}
#[test]
fn exclusive_handles_preserve_private_unicode_files_and_publication_identity() {
    let f = Fixture::new();
    let stage = f.directory.create_dir("stage-鼓").unwrap();
    let mut file = stage.create_file("clé.wav").unwrap();
    file.write_all(b"private samples").unwrap();
    file.sync_all().unwrap();
    verify_private_file(&file).unwrap();
    let identity = file_identity(&file).unwrap();
    assert!(stage.create_file("clé.wav").is_err());
    drop(file);
    let mut file = stage.read_file("clé.wav").unwrap();
    assert_eq!(file_identity(&file).unwrap(), identity);
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"private samples");
    drop(file);
    stage.sync().unwrap();
    f.directory.publish("stage-鼓", &stage, "complete").unwrap();
    assert_eq!(
        f.directory
            .open_dir("complete")
            .unwrap()
            .identity()
            .unwrap(),
        stage.identity().unwrap()
    );
    assert!(f.directory.open_dir("stage-鼓").is_err());
    let other = f.directory.create_dir("other").unwrap();
    assert!(f.directory.publish("other", &other, "complete").is_err());
    assert!(f.directory.open_dir("other").is_ok());
}
#[test]
fn unsafe_child_names_are_identically_rejected_before_io() {
    let f = Fixture::new();
    for name in [
        "",
        ".",
        "..",
        "a/b",
        "a\\b",
        "name:stream",
        "x\0y",
        "NUL",
        "CON.wav",
        "COM1",
        "LPT9.json",
        "trailing.",
        "trailing ",
    ] {
        assert!(f.directory.create_file(name).is_err(), "{name:?}");
        assert!(f.directory.create_dir(name).is_err(), "{name:?}");
    }
    assert!(PrivateDirectory::open_root(std::path::Path::new("relative")).is_err());
    assert!(PrivateDirectory::create_root(&f.path).is_err());
}
#[test]
fn empty_directory_cleanup_checks_identity_and_retains_unknown_contents() {
    let f = Fixture::new();
    let first = f.directory.create_dir("first").unwrap();
    let second = f.directory.create_dir("second").unwrap();
    assert!(f.directory.remove_dir("first", &second).is_err());
    let mut file = first.create_file("owned").unwrap();
    file.write_all(b"data").unwrap();
    drop(file);
    assert!(f.directory.remove_dir("first", &first).is_err());
    first.remove_file("owned").unwrap();
    f.directory.remove_dir("first", &first).unwrap();
    drop(first);
    assert!(f.directory.open_dir("first").is_err());
    assert!(f.directory.open_dir("second").is_ok());
}
#[test]
#[cfg(unix)]
fn symlink_hardlink_fifo_and_broad_permissions_fail_without_repair() {
    use std::os::unix::{
        ffi::OsStrExt,
        fs::{symlink, PermissionsExt},
    };
    let f = Fixture::new();
    let mut file = f.directory.create_file("original").unwrap();
    file.write_all(b"secret").unwrap();
    drop(file);
    symlink("original", f.path.join("alias")).unwrap();
    assert!(f.directory.read_file("alias").is_err());
    assert!(f.directory.remove_file("alias").is_err());
    assert!(f
        .path
        .join("alias")
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
    fs::hard_link(f.path.join("original"), f.path.join("hard")).unwrap();
    assert!(f.directory.read_file("original").is_err());
    assert!(f.directory.read_file("hard").is_err());
    fs::remove_file(f.path.join("hard")).unwrap();
    fs::set_permissions(f.path.join("original"), fs::Permissions::from_mode(0o644)).unwrap();
    assert!(f.directory.read_file("original").is_err());
    assert_eq!(
        fs::metadata(f.path.join("original"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
    let fifo = std::ffi::CString::new(f.path.join("pipe").as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
    assert!(f.directory.read_file("pipe").is_err()); // O_NONBLOCK prevents hanging.
    let child = f.directory.create_dir("broad").unwrap();
    fs::set_permissions(f.path.join("broad"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(f.directory.open_dir("broad").is_err());
    assert!(child.revalidate().is_err());
}
#[test]
#[cfg(unix)]
fn retained_roots_ignore_replaced_path_aliases_and_reject_stale_stages() {
    use std::os::unix::fs::symlink;
    let mut f = Fixture::new();
    let moved = f.path.with_extension("moved");
    fs::rename(&f.path, &moved).unwrap();
    let foreign = Fixture::new();
    symlink(&foreign.path, &f.path).unwrap();
    assert!(PrivateDirectory::open_root(&f.path).is_err());
    let file = f.directory.create_file("rooted").unwrap();
    drop(file);
    assert!(moved.join("rooted").exists());
    assert!(!foreign.path.join("rooted").exists());
    fs::remove_file(&f.path).unwrap();
    f.path = moved;
    let stage = f.directory.create_dir("stage").unwrap();
    fs::rename(f.path.join("stage"), f.path.join("moved-stage")).unwrap();
    f.directory.create_dir("stage").unwrap();
    assert!(f.directory.publish("stage", &stage, "result").is_err());
    assert!(f.directory.remove_dir("stage", &stage).is_err());
    assert!(!f.path.join("result").exists());
    let alias = f.path.join("ancestor");
    symlink(&foreign.path, &alias).unwrap();
    assert!(PrivateDirectory::create_root(&alias.join("escaped")).is_err());
    assert!(!foreign.path.join("escaped").exists());
    assert!(PrivateDirectory::open_root(&f.path.join(".")).is_err());
}

#[test]
#[cfg(target_os = "macos")]
fn darwin_extended_acls_are_rejected_even_when_mode_bits_remain_owner_only() {
    let f = Fixture::new();
    let file = f.directory.create_file("extended").unwrap();
    drop(file);
    assert!(std::process::Command::new("/bin/chmod")
        .args(["+a", "everyone allow read"])
        .arg(f.path.join("extended"))
        .status()
        .unwrap()
        .success());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(f.path.join("extended"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert!(f.directory.read_file("extended").is_err());
    assert!(std::process::Command::new("/bin/chmod")
        .args(["+a", "everyone allow search"])
        .arg(&f.path)
        .status()
        .unwrap()
        .success());
    assert!(f.directory.revalidate().is_err());
    assert!(PrivateDirectory::open_root(&f.path).is_err());
}

#[test]
#[cfg(windows)]
fn protected_acl_children_are_explicit_and_inherited_files_or_hardlinks_reject() {
    let f = Fixture::new();
    let child = f.directory.create_dir("child").unwrap();
    let mut file = child.create_file("protected").unwrap();
    file.write_all(b"private").unwrap();
    file.sync_all().unwrap();
    drop(file);
    child.read_file("protected").unwrap();
    // A normal inherited child ACL is not silently promoted to protected.
    fs::write(f.path.join("child/inherited"), b"unchanged").unwrap();
    assert!(child.read_file("inherited").is_err());
    assert!(child.create_file("inherited").is_err());
    assert_eq!(
        fs::read(f.path.join("child/inherited")).unwrap(),
        b"unchanged"
    );
    fs::hard_link(f.path.join("child/protected"), f.path.join("child/alias")).unwrap();
    assert!(child.read_file("protected").is_err());
    assert!(child.read_file("alias").is_err());
}
