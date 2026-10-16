use crate::Error;
use base64::Engine;
use serde::{Deserialize, Serialize};
use soundroom_client::{
    auth::{hmac_sha256, new_id},
    models::RemoteRole,
    validation::uuid,
};
#[cfg(unix)]
use std::io::{Read, Write};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct Disk {
    version: u32,
    server_id: String,
    server_name: String,
    credentials: Vec<Record>,
}
/// Do not implement Debug: secret-bearing persistence never belongs in logs.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Record {
    pub id: String,
    pub name: String,
    pub role: RemoteRole,
    secret: String,
}
struct Grant {
    name: String,
    role: RemoteRole,
    expires: Instant,
}
pub struct Registry {
    pub server_id: String,
    pub server_name: String,
    records: BTreeMap<String, Record>,
    grants: BTreeMap<String, Grant>,
    path: PathBuf,
}
fn err(s: &str) -> Error {
    Error::Protocol(s.into())
}
pub fn valid_name(name: &str) -> bool {
    !name.trim().is_empty() && name.len() <= 128 && !name.chars().any(char::is_control)
}
#[cfg(unix)]
pub fn private_directory(path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};
    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    let m = std::fs::symlink_metadata(path)?;
    if !m.is_dir()
        || m.file_type().is_symlink()
        || m.uid() != unsafe { libc::getuid() }
        || m.mode() & 0o077 != 0
    {
        return Err(err("Service storage must be an owner-only real directory"));
    }
    Ok(())
}
#[cfg(not(any(unix, windows)))]
pub fn private_directory(_path: &Path) -> Result<(), Error> {
    Err(err(
        "An ACL-qualified storage provider is required on this platform",
    ))
}
#[cfg(unix)]
pub fn read_private(path: &Path, max: u64) -> Result<Vec<u8>, Error> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let m = f.metadata()?;
    if !m.is_file()
        || m.uid() != unsafe { libc::getuid() }
        || m.mode() & 0o077 != 0
        || m.nlink() != 1
        || m.len() > max
    {
        return Err(err("Owner-only regular file is required"));
    }
    let mut bytes = vec![];
    f.take(max + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max {
        return Err(err("Private file exceeds its budget"));
    }
    Ok(bytes)
}
#[cfg(not(any(unix, windows)))]
pub fn read_private(_path: &Path, _max: u64) -> Result<Vec<u8>, Error> {
    Err(err(
        "An ACL-qualified credential provider is required on this platform",
    ))
}
#[cfg(unix)]
pub fn write_new_private(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    if let Err(e) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = std::fs::remove_file(path);
        return Err(e.into());
    }
    Ok(())
}
#[cfg(not(any(unix, windows)))]
pub fn write_new_private(_path: &Path, _bytes: &[u8]) -> Result<(), Error> {
    Err(err(
        "An ACL-qualified credential store is required on this platform",
    ))
}
#[cfg(windows)]
pub use soundroom_client::windows_storage::{private_directory, read_private, write_new_private};
impl Registry {
    pub fn create(root: &Path, name: &str) -> Result<Self, Error> {
        if !valid_name(name) {
            return Err(err("Invalid server name"));
        }
        private_directory(root)?;
        let out = Self {
            server_id: new_id()?,
            server_name: name.into(),
            records: BTreeMap::new(),
            grants: BTreeMap::new(),
            path: root.join("registry.json"),
        };
        write_new_private(&out.path, &serde_json::to_vec(&out.disk())?)?;
        Ok(out)
    }
    pub fn load(root: &Path) -> Result<Self, Error> {
        private_directory(root)?;
        let path = root.join("registry.json");
        let disk: Disk = serde_json::from_slice(&read_private(&path, 1024 * 1024)?)?;
        if disk.version != 3
            || !uuid(&disk.server_id)
            || !valid_name(&disk.server_name)
            || disk.credentials.len() > 256
        {
            return Err(err("Invalid credential registry"));
        }
        let mut records = BTreeMap::new();
        for r in disk.credentials {
            if !uuid(&r.id)
                || !valid_name(&r.name)
                || base64::engine::general_purpose::STANDARD
                    .decode(&r.secret)
                    .map_or(true, |v| v.len() != 32)
                || records.insert(r.id.to_ascii_uppercase(), r).is_some()
            {
                return Err(err("Invalid credential record"));
            }
        }
        Ok(Self {
            server_id: disk.server_id.to_ascii_uppercase(),
            server_name: disk.server_name,
            records,
            grants: BTreeMap::new(),
            path,
        })
    }
    fn disk(&self) -> Disk {
        Disk {
            version: 3,
            server_id: self.server_id.clone(),
            server_name: self.server_name.clone(),
            credentials: self.records.values().cloned().collect(),
        }
    }
    fn persist(&self) -> Result<(), Error> {
        let stage = self.path.with_extension(format!("{}.tmp", new_id()?));
        write_new_private(&stage, &serde_json::to_vec(&self.disk())?)?;
        #[cfg(windows)]
        let replacement = soundroom_client::windows_storage::replace_private(&stage, &self.path);
        #[cfg(not(windows))]
        let replacement = std::fs::rename(&stage, &self.path).map_err(Error::from);
        if let Err(e) = replacement {
            let _ = std::fs::remove_file(stage);
            return Err(e);
        }
        #[cfg(unix)]
        std::fs::File::open(self.path.parent().unwrap())?.sync_all()?;
        Ok(())
    }
    pub fn issue(&mut self, name: &str, role: RemoteRole) -> Result<serde_json::Value, Error> {
        if !valid_name(name) || self.records.len() >= 256 {
            return Err(err("Invalid name or registry is full"));
        }
        let id = new_id()?;
        let mut secret = [0u8; 32];
        random_secret(&mut secret)?;
        let r = Record {
            id: id.clone(),
            name: name.into(),
            role: role.clone(),
            secret: base64::engine::general_purpose::STANDARD.encode(secret),
        };
        secret.fill(0);
        self.records.insert(id.clone(), r.clone());
        if let Err(e) = self.persist() {
            self.records.remove(&id);
            return Err(e);
        }
        Ok(
            serde_json::json!({"version":3,"serverID":self.server_id,"credentialID":id,"clientSecret":r.secret,"role":role}),
        )
    }
    pub fn verify(
        &self,
        server: &str,
        id: &str,
        peer: &str,
        name: &str,
        nonce: &str,
        proof: &str,
    ) -> Result<Record, Error> {
        if !server.eq_ignore_ascii_case(&self.server_id)
            || !uuid(id)
            || !uuid(peer)
            || !valid_name(name)
        {
            return Err(err("Authentication failed"));
        }
        let record = self
            .records
            .get(&id.to_ascii_uppercase())
            .filter(|r| r.name == name)
            .ok_or_else(|| err("Authentication failed"))?;
        let mut key = base64::engine::general_purpose::STANDARD
            .decode(&record.secret)
            .map_err(|_| err("Authentication failed"))?;
        let message = format!(
            "soundroom-auth-v2|{}|{}|{}|{}|{}",
            self.server_id,
            id.to_ascii_uppercase(),
            peer.to_ascii_uppercase(),
            name,
            nonce
        );
        let expected = hmac_sha256(&key, message.as_bytes());
        key.fill(0);
        let supplied = base64::engine::general_purpose::STANDARD
            .decode(proof)
            .map_err(|_| err("Authentication failed"))?;
        let mut difference = (supplied.len() != 32) as u8;
        for (i, b) in expected.iter().enumerate() {
            difference |= *b ^ supplied.get(i).copied().unwrap_or(0);
        }
        if difference != 0 {
            return Err(err("Authentication failed"));
        }
        Ok(record.clone())
    }
    pub fn grant(&mut self, name: &str, role: RemoteRole) -> Result<serde_json::Value, Error> {
        self.grant_with_lifetime(name, role, 120)
    }
    pub fn grant_with_lifetime(
        &mut self,
        name: &str,
        role: RemoteRole,
        seconds: u64,
    ) -> Result<serde_json::Value, Error> {
        if !(120..=3600).contains(&seconds) {
            return Err(err("Pairing code lifetime must be 2–60 minutes"));
        }
        self.grants.retain(|_, g| g.expires > Instant::now());
        if !valid_name(name) || self.grants.len() >= 16 {
            return Err(err("Invalid pairing name or too many pending grants"));
        }
        let mut bytes = [0u8; 32];
        random_secret(&mut bytes)?;
        let code = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        bytes.fill(0);
        self.grants.insert(
            code.clone(),
            Grant {
                name: name.into(),
                role: role.clone(),
                expires: Instant::now() + Duration::from_secs(seconds),
            },
        );
        Ok(serde_json::json!({"code":code,"name":name,"role":role,"expiresInSeconds":seconds}))
    }
    pub fn redeem(&mut self, code: &str, name: &str) -> Result<serde_json::Value, Error> {
        // Consume even an invalid attempted name: a code is never reusable after submission.
        let grant = self
            .grants
            .remove(code)
            .ok_or_else(|| err("Pairing code is invalid or expired"))?;
        if grant.expires < Instant::now() || grant.name != name {
            return Err(err("Pairing code is invalid or expired"));
        }
        self.issue(name, grant.role)
    }
    pub fn revoke(&mut self, id: &str) -> Result<(), Error> {
        let record = self
            .records
            .remove(&id.to_ascii_uppercase())
            .ok_or_else(|| err("Unknown credential"))?;
        if let Err(e) = self.persist() {
            self.records.insert(record.id.clone(), record);
            return Err(e);
        }
        Ok(())
    }
    pub fn authorizes(&self, id: &str, role: &RemoteRole) -> bool {
        self.records
            .get(id)
            .is_some_and(|record| &record.role == role)
    }
    pub fn contains(&self, id: &str) -> bool {
        self.records.contains_key(id)
    }
    pub fn clients(&self) -> serde_json::Value {
        serde_json::Value::Array(
            self.records
                .values()
                .map(|r| serde_json::json!({"id":r.id,"name":r.name,"role":r.role}))
                .collect(),
        )
    }
}
fn random_secret(out: &mut [u8; 32]) -> Result<(), Error> {
    // UUID generator uses the same operating-system random source; use direct random here.
    soundroom_client::auth::fill_random(out)
}
/// One process owns a storage root. Operator CLI commands cannot race a live registry.
pub struct RootLock {
    _file: std::fs::File,
}
impl RootLock {
    #[cfg(unix)]
    pub fn acquire(root: &Path) -> Result<Self, Error> {
        use std::os::unix::{
            fs::{MetadataExt, OpenOptionsExt},
            io::AsRawFd,
        };
        private_directory(root)?;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(root.join("service.lock"))?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::getuid() }
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            return Err(err(
                "Service lock must be an owner-only single-link regular file",
            ));
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(err(
                "Another service/operator process owns this storage root",
            ));
        }
        Ok(Self { _file: file })
    }
    #[cfg(windows)]
    pub fn acquire(root: &Path) -> Result<Self, Error> {
        private_directory(root)?;
        Ok(Self {
            _file: soundroom_client::windows_storage::exclusive_lock(&root.join("service.lock"))?,
        })
    }
    #[cfg(not(any(unix, windows)))]
    pub fn acquire(_root: &Path) -> Result<Self, Error> {
        Err(err(
            "An ACL-qualified exclusive storage lock is required on this platform",
        ))
    }
}
