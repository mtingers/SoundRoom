use crate::{models::RemoteRole, validation::uuid, Error};
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
/// Secret-bearing object deliberately does not implement Debug or Serialize.
#[derive(Clone)]
pub struct Credential {
    pub server_id: String,
    pub credential_id: String,
    pub role: RemoteRole,
    client_secret: [u8; 32],
}
impl Credential {
    /// The caller must acquire these bytes from a protected credential source.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.is_empty() || bytes.len() > 16384 {
            return Err(Error::Protocol("Invalid credential file".into()));
        }
        let object: Value = serde_json::from_slice(bytes)?;
        let version = object["version"]
            .as_u64()
            .ok_or_else(|| Error::Protocol("Invalid credential version".into()))?;
        if ![1, 3].contains(&version) {
            return Err(Error::Protocol("Unsupported credential version".into()));
        }
        let server_id = object["serverID"]
            .as_str()
            .filter(|v| uuid(v))
            .ok_or_else(|| Error::Protocol("Invalid credential identity".into()))?
            .to_ascii_uppercase();
        let credential_id = object[if version == 1 {
            "clientID"
        } else {
            "credentialID"
        }]
        .as_str()
        .filter(|v| uuid(v))
        .ok_or_else(|| Error::Protocol("Invalid credential identity".into()))?
        .to_ascii_uppercase();
        let secret = base64::engine::general_purpose::STANDARD
            .decode(object["clientSecret"].as_str().unwrap_or(""))
            .map_err(|_| Error::Protocol("Invalid credential secret".into()))?;
        let client_secret: [u8; 32] = secret
            .try_into()
            .map_err(|_| Error::Protocol("Invalid credential secret length".into()))?;
        let role: RemoteRole = serde_json::from_value(object["role"].clone())?;
        Ok(Self {
            server_id,
            credential_id,
            role,
            client_secret,
        })
    }
    #[cfg(unix)]
    pub fn load(path: &std::path::Path) -> Result<Self, Error> {
        use std::{
            io::Read,
            os::unix::fs::{MetadataExt, OpenOptionsExt},
        };
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != unsafe { libc::getuid() }
            || metadata.mode() & 0o077 != 0
            || metadata.nlink() != 1
            || metadata.len() == 0
            || metadata.len() > 16384
        {
            return Err(Error::Protocol(
                "Credential must be an owner-only regular file".into(),
            ));
        }
        let mut bytes = Vec::new();
        file.take(16385).read_to_end(&mut bytes)?;
        Self::from_bytes(&bytes)
    }
    #[cfg(windows)]
    pub fn load(path: &std::path::Path) -> Result<Self, Error> {
        Self::from_bytes(&crate::windows_storage::read_private(path, 16384)?)
    }
    #[cfg(not(any(unix, windows)))]
    pub fn load(_path: &std::path::Path) -> Result<Self, Error> {
        Err(Error::Protocol(
            "Use an ACL-verified credential provider on this platform".into(),
        ))
    }
    /// Redeems an operator-approved, one-time code over the verified WSS channel.
    pub fn pair(
        config: &crate::transport::WssConfig,
        code: &str,
        name: &str,
    ) -> Result<Self, Error> {
        use crate::transport::ControlTransport;
        if code.len() != 43
            || !code
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || name.is_empty()
            || name.len() > 128
        {
            return Err(Error::Protocol("Invalid approved pairing code/name".into()));
        }
        let mut transport = crate::transport::WssTransport::connect(config)?;
        let challenge = transport
            .receive(std::time::Duration::from_secs(10))?
            .ok_or(Error::Timeout)?;
        let challenge: Value = serde_json::from_slice(&challenge)?;
        let server_id = challenge["serverID"]
            .as_str()
            .filter(|id| uuid(id))
            .ok_or_else(|| Error::Protocol("Invalid pairing challenge identity".into()))?;
        if challenge["version"] != 3 || challenge["kind"] != "challenge" {
            return Err(Error::Protocol("Expected pairing challenge".into()));
        }
        transport.send(&serde_json::to_vec(
            &json!({"version":3,"kind":"pair","code":code,"name":name}),
        )?)?;
        let response = transport
            .receive(std::time::Duration::from_secs(10))?
            .ok_or(Error::Timeout)?;
        let response: Value = serde_json::from_slice(&response)?;
        if response["version"] != 3
            || response["kind"] != "paired"
            || response
                .as_object()
                .map_or(true, |object| object.len() != 3)
        {
            return Err(Error::Protocol(
                "Server rejected the approved pairing code".into(),
            ));
        }
        crate::validation::validate_model(&response["credential"], "RemoteBrowserCredential")?;
        let credential = Self::from_bytes(&serde_json::to_vec(&response["credential"])?)?;
        if !credential.server_id.eq_ignore_ascii_case(server_id) {
            return Err(Error::Protocol("Pairing server identity changed".into()));
        }
        transport.close()?;
        Ok(credential)
    }
    #[cfg(unix)]
    pub fn write_new(&self, path: &std::path::Path) -> Result<(), Error> {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let bytes = serde_json::to_vec(
            &json!({"version":3,"serverID":self.server_id,"credentialID":self.credential_id,"clientSecret":base64::engine::general_purpose::STANDARD.encode(self.client_secret),"role":self.role}),
        )?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
            let _ = std::fs::remove_file(path);
            return Err(error.into());
        }
        Ok(())
    }
    #[cfg(windows)]
    pub fn write_new(&self, path: &std::path::Path) -> Result<(), Error> {
        let bytes = serde_json::to_vec(
            &json!({"version":3,"serverID":self.server_id,"credentialID":self.credential_id,"clientSecret":base64::engine::general_purpose::STANDARD.encode(self.client_secret),"role":self.role}),
        )?;
        crate::windows_storage::write_new_private(path, &bytes)
    }
    pub(crate) fn hello(
        &self,
        client_id: &str,
        name: &str,
        challenge: &Value,
    ) -> Result<Value, Error> {
        if challenge["version"] != 3
            || challenge["kind"] != "challenge"
            || challenge["serverID"]
                .as_str()
                .map(str::to_ascii_uppercase)
                .as_deref()
                != Some(&self.server_id)
        {
            return Err(Error::Protocol(
                "Challenge server identity does not match the credential".into(),
            ));
        }
        let nonce = challenge["nonce"]
            .as_str()
            .ok_or_else(|| Error::Protocol("Missing authentication challenge".into()))?;
        if base64::engine::general_purpose::STANDARD
            .decode(nonce)
            .map_err(|_| Error::Protocol("Invalid challenge nonce".into()))?
            .len()
            != 32
        {
            return Err(Error::Protocol("Invalid challenge nonce length".into()));
        }
        if !uuid(client_id)
            || name.is_empty()
            || name.len() > 128
            || name.chars().any(char::is_control)
        {
            return Err(Error::Protocol("Invalid client identity/name".into()));
        }
        let message = format!(
            "soundroom-auth-v2|{}|{}|{}|{}|{}",
            self.server_id,
            self.credential_id,
            client_id.to_ascii_uppercase(),
            name,
            nonce
        );
        let proof = base64::engine::general_purpose::STANDARD
            .encode(hmac_sha256(&self.client_secret, message.as_bytes()));
        Ok(
            json!({"version":3,"kind":"hello","serverID":self.server_id,"credentialID":self.credential_id,"peer":{"id":client_id,"name":name,"role":self.role},"proof":proof}),
        )
    }
}
impl Drop for Credential {
    fn drop(&mut self) {
        self.client_secret.fill(0);
    }
}
/// Standard HMAC construction over the maintained SHA-256 primitive.
pub fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    let mut outer = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    inner.update(message);
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    block.fill(0);
    outer.finalize().into()
}
pub fn new_id() -> Result<String, Error> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes)
        .map_err(|_| Error::Protocol("Operating-system randomness is unavailable".into()))?;
    bytes[6] = (bytes[6] & 15) | 64;
    bytes[8] = (bytes[8] & 63) | 128;
    let h: Vec<_> = bytes.iter().map(|b| format!("{b:02X}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        h[0..4].concat(),
        h[4..6].concat(),
        h[6..8].concat(),
        h[8..10].concat(),
        h[10..16].concat()
    ))
}

/// Operating-system randomness for protocol nonces and pairing secrets.
pub fn fill_random(bytes: &mut [u8]) -> Result<(), Error> {
    getrandom::getrandom(bytes)
        .map_err(|_| Error::Protocol("Operating-system randomness is unavailable".into()))
}
