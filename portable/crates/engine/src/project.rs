use crate::{
    model::Session,
    wave::{WaveInfo, WaveReader},
    Error, Result,
};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};
pub const METADATA_LIMIT: u64 = 64 * 1024 * 1024;
#[derive(Debug)]
pub struct Project {
    pub session: Session,
    pub media: BTreeMap<String, Media>,
}
#[derive(Clone, Debug)]
pub struct Media {
    pub path: PathBuf,
    pub info: WaveInfo,
}
impl Project {
    pub fn open(path: &Path) -> Result<Self> {
        if !path.is_dir() {
            return Err(Error::Invalid(
                "A .soundroom project must be a directory containing session.json and Audio/"
                    .into(),
            ));
        }
        reject_symlink(path)?;
        let metadata = path.join("session.json");
        reject_symlink(&metadata)?;
        let file = fs::File::open(&metadata)?;
        if file.metadata()?.len() > METADATA_LIMIT {
            return Err(Error::Invalid(
                "Project metadata exceeds the current 64 MiB portable loader bound".into(),
            ));
        }
        let mut bytes = vec![];
        file.take(METADATA_LIMIT + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > METADATA_LIMIT {
            return Err(Error::Invalid("Project metadata grew beyond bound".into()));
        }
        let session: Session = serde_json::from_slice(&bytes)?;
        session.validate()?;
        let audio = path.join("Audio");
        reject_symlink(&audio)?;
        let mut media = BTreeMap::new();
        for id in session.asset_ids() {
            let canonical = audio.join(format!("{id}.wav"));
            let lower = audio.join(format!("{}.wav", id.to_lowercase()));
            let file = if canonical.exists() { canonical } else { lower };
            reject_symlink(&file)?;
            let reader = WaveReader::open(&file)?;
            if reader.info.sample_rate != 48000 {
                return Err(Error::Unsupported(format!(
                    "Project media {id} is not 48 kHz; offline import resampling is required"
                )));
            }
            media.insert(
                id,
                Media {
                    path: file,
                    info: reader.info,
                },
            );
        }
        let project = Self { session, media };
        project.validate_sources()?;
        Ok(project)
    }
    pub fn validate_sources(&self) -> Result<()> {
        self.session.validate()?;
        for c in self.session.tracks.iter().flat_map(|t| {
            t.clips
                .iter()
                .chain(t.recording_takes.iter().flat_map(|v| &v.clips))
        }) {
            self.check_source(&c.asset_id, c.offset, c.length)?;
            for h in &c.previous_states {
                self.check_source(&h.asset_id, h.offset, h.length)?
            }
        }
        Ok(())
    }
    fn check_source(&self, id: &str, offset: u64, length: u64) -> Result<()> {
        let media = self
            .media
            .get(&id.to_uppercase())
            .ok_or_else(|| Error::Invalid(format!("Missing source {id}")))?;
        if offset > media.info.frames || length > media.info.frames - offset {
            return Err(Error::Invalid(format!("Clip exceeds source {id}")));
        }
        Ok(())
    }
    /// Save to a new destination. Never overwrite a live project directory, even transiently.
    /// Use `save(target, true)` for explicit atomic project replacement.
    pub fn save_new(&self, target: &Path) -> Result<()> {
        self.save(target, false)
    }
    /// Stage a complete validated package before atomically replacing a saved
    /// project. Unreferenced audio is retained until explicit media cleanup.
    pub fn save(&self, target: &Path, replace_existing: bool) -> Result<()> {
        self.validate_sources()?;
        let previous = match fs::symlink_metadata(target) {
            Ok(metadata) => {
                if !replace_existing {
                    return Err(Error::Conflict("A project with this name already exists. Choose another name or explicitly replace it.".into()));
                }
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(Error::Conflict(
                        "The save destination is not a regular Soundroom project directory".into(),
                    ));
                }
                Self::open(target)?;
                Some(metadata)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let parent = target.parent().unwrap_or(Path::new("."));
        fs::create_dir_all(parent)?;
        let suffix = format!(
            ".soundroom-stage-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| Error::Invalid("Clock before epoch".into()))?
                .as_nanos()
        );
        let stage = parent.join(suffix);
        fs::create_dir(&stage)?;
        let result = (|| {
            fs::create_dir(stage.join("Audio"))?;
            let mut media_info = vec![];
            for id in self.session.asset_ids() {
                let media = &self.media[&id];
                let file = stage.join("Audio").join(format!("{id}.wav"));
                if fs::hard_link(&media.path, &file).is_err() {
                    fs::copy(&media.path, &file)?;
                }
                media_info.push(serde_json::json!(id));
                media_info.push(
                    serde_json::json!({"frames":media.info.frames,"channels":media.info.channels}),
                );
            }
            if previous.is_some() {
                for entry in fs::read_dir(target.join("Audio"))? {
                    let entry = entry?;
                    let kind = entry.file_type()?;
                    if !kind.is_file() {
                        return Err(Error::Invalid(
                            "Saved project audio contains a link or nonregular file".into(),
                        ));
                    }
                    let destination = stage.join("Audio").join(entry.file_name());
                    if !destination.exists() && fs::hard_link(entry.path(), &destination).is_err() {
                        fs::copy(entry.path(), destination)?;
                    }
                }
            }
            let mut document = self.session.clone();
            document.version = document.required_version();
            if document.timing != Default::default() {
                document.version = document.version.max(9)
            }
            let metadata = serde_json::to_vec_pretty(&document)?;
            if metadata.len() as u64 > METADATA_LIMIT {
                return Err(Error::Invalid("Saved metadata exceeds loader bound".into()));
            }
            fs::write(stage.join("session.json"), metadata)?;
            fs::write(
                stage.join("media.json"),
                serde_json::to_vec_pretty(&media_info)?,
            )?;
            for file in ["session.json", "media.json"] {
                fs::OpenOptions::new()
                    .write(true)
                    .open(stage.join(file))?
                    .sync_all()?
            }
            for entry in fs::read_dir(stage.join("Audio"))? {
                fs::File::open(entry?.path())?.sync_all()?;
            }
            #[cfg(unix)]
            for directory in [stage.join("Audio"), stage.clone()] {
                fs::File::open(directory)?.sync_all()?;
            }
            // Revalidate the old package identity/metadata just before the
            // exchange. Do not replace a destination changed during staging.
            if let Some(previous) = &previous {
                let current = fs::symlink_metadata(target)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if current.dev() != previous.dev() || current.ino() != previous.ino() {
                        return Err(Error::Conflict(
                            "The project destination changed during save. Retry saving.".into(),
                        ));
                    }
                }
                if !current.is_dir() || current.file_type().is_symlink() {
                    return Err(Error::Conflict(
                        "The project destination changed during save".into(),
                    ));
                }
                crate::storage::replace_directory(&stage, target)?;
            } else {
                crate::storage::commit_new_directory(&stage, target)?;
            }
            // Publication succeeded. A cleanup failure must not turn a
            // completed save into an error or remove the committed destination.
            let _ = fs::remove_dir_all(&stage);
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(stage);
        }
        result
    }
}
fn reject_symlink(path: &Path) -> Result<()> {
    if fs::symlink_metadata(path)?.file_type().is_symlink() {
        Err(Error::Invalid(format!(
            "Symlink project path is not accepted: {}",
            path.display()
        )))
    } else {
        Ok(())
    }
}
