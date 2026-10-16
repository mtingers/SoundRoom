use crate::{
    models::{Kind, RemoteSnapshot, RemoteSnapshotPage},
    validation::{validate_model, MAX_SNAPSHOT_BYTES},
    Error,
};
use base64::Engine;
use sha2::{Digest, Sha256};
#[derive(Default)]
pub struct SnapshotAssembler {
    begin: Option<RemoteSnapshotPage>,
    bytes: Vec<u8>,
}
impl SnapshotAssembler {
    pub fn active(&self) -> bool {
        self.begin.is_some()
    }
    pub fn next_offset(&self) -> usize {
        self.bytes.len()
    }
    pub fn revision(&self) -> Option<i64> {
        self.begin.as_ref().map(|p| p.revision)
    }
    pub fn cancel(&mut self) {
        self.begin = None;
        self.bytes = Vec::new();
    }
    pub fn accept(&mut self, page: RemoteSnapshotPage) -> Result<Option<RemoteSnapshot>, Error> {
        let result = self.accept_inner(page);
        if result.is_err() {
            self.cancel();
        }
        result
    }
    fn accept_inner(&mut self, page: RemoteSnapshotPage) -> Result<Option<RemoteSnapshot>, Error> {
        validate_model(&serde_json::to_value(&page)?, "RemoteSnapshotPage")?;
        let fail = || Error::Protocol("Invalid revision-pinned snapshot transfer".into());
        if page.version != 3 || page.revision < 0 {
            return Err(fail());
        }
        match page.kind {
            Kind::SnapshotBegin => {
                let size = page
                    .total_bytes
                    .filter(|v| *v > 0 && *v <= MAX_SNAPSHOT_BYTES as i64)
                    .ok_or_else(fail)?;
                let digest = page
                    .sha256
                    .as_ref()
                    .filter(|v| {
                        v.len() == 64
                            && v.bytes()
                                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    })
                    .ok_or_else(fail)?;
                let _ = digest;
                if page.data.is_some() || page.offset.is_some() {
                    return Err(fail());
                }
                self.bytes = Vec::with_capacity((size as usize).min(524288));
                self.begin = Some(page);
                Ok(None)
            }
            Kind::SnapshotPage => {
                let begin = self.begin.as_ref().ok_or_else(fail)?;
                if page.transfer_id != begin.transfer_id
                    || page.revision != begin.revision
                    || page.offset != Some(self.bytes.len() as i64)
                    || page.sha256.is_some()
                    || page.total_bytes.is_some()
                {
                    return Err(fail());
                }
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(page.data.as_deref().ok_or_else(fail)?)
                    .map_err(|_| fail())?;
                if bytes.is_empty()
                    || bytes.len() > 524288
                    || self.bytes.len() + bytes.len() > begin.total_bytes.unwrap() as usize
                {
                    return Err(fail());
                }
                self.bytes.extend_from_slice(&bytes);
                Ok(None)
            }
            Kind::SnapshotEnd => {
                let begin = self.begin.as_ref().ok_or_else(fail)?;
                if page.transfer_id != begin.transfer_id
                    || page.revision != begin.revision
                    || page.total_bytes != begin.total_bytes
                    || page.sha256 != begin.sha256
                    || page.offset.is_some()
                    || page.data.is_some()
                    || self.bytes.len() != begin.total_bytes.unwrap() as usize
                {
                    return Err(fail());
                }
                let digest = Sha256::digest(&self.bytes)
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>();
                if Some(&digest) != begin.sha256.as_ref() {
                    return Err(fail());
                }
                let snapshot: RemoteSnapshot =
                    crate::validation::decode_model(&self.bytes, "RemoteSnapshot")?;
                if snapshot.revision != begin.revision {
                    return Err(fail());
                }
                self.cancel();
                Ok(Some(snapshot))
            }
        }
    }
}
