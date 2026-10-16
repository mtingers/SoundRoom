#![cfg(windows)]
use soundroom_client::{
    auth::{new_id, Credential},
    windows_storage as storage,
};
use std::path::PathBuf;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("soundroom-owner-acl-{}", new_id().unwrap()));
        storage::private_directory(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn owner_files_support_unicode_and_bound_reads_without_overwrite() {
    let f = Fixture::new();
    let path = f.0.join("clé-鼓.json");
    storage::write_new_private(&path, b"private").unwrap();
    assert_eq!(storage::read_private(&path, 7).unwrap(), b"private");
    assert!(storage::read_private(&path, 6).is_err());
    assert!(storage::write_new_private(&path, b"overwrite").is_err());
    let file = std::fs::File::open(&path).unwrap();
    storage::verify_private_file(&file).unwrap();
}
#[test]
fn broad_or_inherited_dacls_are_rejected_without_repair() {
    let f = Fixture::new();
    let path = f.0.join("inherited.json");
    std::fs::write(&path, b"not protected").unwrap();
    assert!(storage::read_private(&path, 1024).is_err());
    // Existing inherited ACLs stay unchanged; acquisition never repairs an unsafe source.
    assert!(storage::write_new_private(&path, b"secret").is_err());
}
#[test]
fn single_link_and_exclusive_lock_are_enforced() {
    let f = Fixture::new();
    let path = f.0.join("source.json");
    storage::write_new_private(&path, b"private").unwrap();
    std::fs::hard_link(&path, f.0.join("alias.json")).unwrap();
    assert!(storage::read_private(&path, 1024).is_err());
    let path = f.0.join("service.lock");
    let first = storage::exclusive_lock(&path).unwrap();
    assert!(storage::exclusive_lock(&path).is_err());
    drop(first);
    storage::exclusive_lock(&path).unwrap();
}
#[test]
fn sibling_replacement_retains_owner_security_and_rejects_cross_directory() {
    let f = Fixture::new();
    let destination = f.0.join("registry.json");
    let stage = f.0.join("stage.json");
    storage::write_new_private(&destination, b"old").unwrap();
    storage::write_new_private(&stage, b"new").unwrap();
    storage::replace_private(&stage, &destination).unwrap();
    assert_eq!(storage::read_private(&destination, 1024).unwrap(), b"new");
    assert!(!stage.exists());
    assert!(
        storage::replace_private(&f.0.join("stage.json"), &f.0.join("other/registry.json"))
            .is_err()
    );
}
#[test]
fn credential_api_uses_windows_provider() {
    let f = Fixture::new();
    let path = f.0.join("credential.json");
    let credential=Credential::from_bytes(br#"{"version":3,"serverID":"AAAAAAAA-AAAA-4AAA-8AAA-AAAAAAAAAAAA","credentialID":"BBBBBBBB-BBBB-4BBB-8BBB-BBBBBBBBBBBB","clientSecret":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=","role":"viewer"}"#).unwrap();
    credential.write_new(&path).unwrap();
    let loaded = Credential::load(&path).unwrap();
    assert_eq!(loaded.server_id, credential.server_id);
    assert_eq!(loaded.credential_id, credential.credential_id);
}
