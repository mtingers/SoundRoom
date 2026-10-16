//! Local file paths never cross the wire. Transfers are bounded, receipt-driven,
//! hash-checked and never automatically replay uncertain mutation requests.
use crate::{
    auth::new_id,
    client::Client,
    models::{RemoteArtifact, RemoteArtifactChunk, RemoteCommand, RemoteReply, RemoteReplyStatus},
    transport::ControlTransport,
    validation, Error,
};
use base64::Engine;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::Path,
    time::Duration,
};
const CHUNK: usize = 262144;
impl<T: ControlTransport> Client<T> {
    pub fn upload_file(
        &mut self,
        path: &Path,
        track_id: Option<String>,
        frame: i64,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<RemoteReply, Error> {
        let mut source = std::fs::File::open(path)?;
        let metadata = source.metadata()?;
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 2_147_483_648 {
            return Err(Error::Protocol(
                "Import requires a regular file of 1 byte to 2 GiB".into(),
            ));
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|n| !n.is_empty() && n.len() <= 256)
            .ok_or_else(|| Error::Protocol("Import filename is invalid".into()))?
            .to_owned();
        let expected = track_id
            .as_ref()
            .and_then(|id| {
                self.snapshot
                    .as_ref()?
                    .track_revisions
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(id))
                    .map(|(_, version)| (id.clone(), *version))
            })
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let digest = hash_reader(&mut source)?;
        use std::io::Seek;
        source.rewind()?;
        let id = new_id()?;
        let reply = self.request(
            RemoteCommand::BeginUpload {
                id: id.clone(),
                name,
                bytes: metadata.len() as i64,
                sha256: digest,
            },
            None,
            BTreeMap::new(),
            Duration::from_secs(30),
        )?;
        require_accepted(&reply)?;
        let mut offset = 0u64;
        let mut buffer = vec![0u8; CHUNK];
        loop {
            let read = source.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            if offset + read as u64 > metadata.len() {
                return Err(Error::Protocol(
                    "Import file changed during transfer".into(),
                ));
            }
            let reply = self.request(
                RemoteCommand::UploadChunk {
                    id: id.clone(),
                    offset: offset as i64,
                    data: base64::engine::general_purpose::STANDARD.encode(&buffer[..read]),
                },
                None,
                BTreeMap::new(),
                Duration::from_secs(30),
            )?;
            require_accepted(&reply)?;
            offset += read as u64;
            progress(offset, metadata.len());
        }
        if offset != metadata.len() {
            return Err(Error::Protocol(
                "Import file changed during transfer".into(),
            ));
        }
        let expected_revision = if track_id.is_none() {
            self.snapshot.as_ref().map(|s| s.revision)
        } else {
            None
        };
        let reply = self.request(
            RemoteCommand::FinishUpload {
                id,
                track_id,
                frame,
            },
            expected_revision,
            expected,
            Duration::from_secs(120),
        )?;
        require_accepted(&reply)?;
        Ok(reply)
    }
    /// Creates a new destination atomically. A failed download removes its partial file.
    pub fn download_artifact(
        &mut self,
        artifact: &RemoteArtifact,
        destination: &Path,
        mut progress: impl FnMut(u64, u64),
    ) -> Result<(), Error> {
        validation::validate_model(&serde_json::to_value(artifact)?, "RemoteArtifact")?;
        if artifact.bytes < 0
            || artifact.bytes > 2_147_483_648
            || artifact.sha256.len() != 64
            || !artifact
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::Protocol("Invalid artifact manifest".into()));
        }
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let temporary = parent.join(format!(".soundroom-download-{}.partial", new_id()?));
        let mut partial = PartialFile {
            path: temporary.clone(),
        };
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let mut digest = Sha256::new();
        let mut offset = 0u64;
        while offset < artifact.bytes as u64 {
            let wanted = CHUNK.min(artifact.bytes as usize - offset as usize);
            let reply = self.request(
                RemoteCommand::DownloadArtifact {
                    id: artifact.id.clone(),
                    offset: offset as i64,
                    bytes: wanted as i64,
                },
                None,
                BTreeMap::new(),
                Duration::from_secs(30),
            )?;
            require_accepted(&reply)?;
            let payload = base64::engine::general_purpose::STANDARD
                .decode(
                    reply
                        .payload
                        .as_deref()
                        .ok_or_else(|| Error::Protocol("Missing artifact chunk".into()))?,
                )
                .map_err(|_| Error::Protocol("Invalid artifact chunk".into()))?;
            let chunk: RemoteArtifactChunk =
                validation::decode_model(&payload, "RemoteArtifactChunk")?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(&chunk.data)
                .map_err(|_| Error::Protocol("Invalid artifact bytes".into()))?;
            if chunk.offset != offset as i64
                || bytes.is_empty()
                || bytes.len() > wanted
                || chunk.complete != (offset + bytes.len() as u64 == artifact.bytes as u64)
            {
                return Err(Error::Protocol(
                    "Artifact offset/completion mismatch".into(),
                ));
            }
            file.write_all(&bytes)?;
            digest.update(&bytes);
            offset += bytes.len() as u64;
            progress(offset, artifact.bytes as u64);
        }
        let actual = digest
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        if actual != artifact.sha256 {
            return Err(Error::Protocol("Artifact SHA-256 does not match".into()));
        }
        file.sync_all()?;
        drop(file);
        // hard_link provides atomic no-clobber installation, unlike exists+rename.
        std::fs::hard_link(&temporary, destination)?;
        std::fs::remove_file(&temporary)?;
        partial.path = std::path::PathBuf::new();
        Ok(())
    }
}
fn hash_reader(reader: &mut impl Read) -> Result<String, Error> {
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn require_accepted(reply: &RemoteReply) -> Result<(), Error> {
    match reply.status {
        RemoteReplyStatus::Accepted => Ok(()),
        RemoteReplyStatus::Conflict => Err(Error::Conflict(reply.message.clone())),
        _ => Err(Error::Protocol(reply.message.clone())),
    }
}
struct PartialFile {
    path: std::path::PathBuf,
}
impl Drop for PartialFile {
    fn drop(&mut self) {
        if !self.path.as_os_str().is_empty() {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
