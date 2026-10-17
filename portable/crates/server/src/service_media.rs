//! Loaded projects retain their own files. Only custody links under Media/ may
//! be removed when the working session no longer references an asset.
use super::{
    edits::{stamp, AssetStamp},
    hash_file, new_id, protocol, Error,
};
use soundroom_engine::project::Media;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

pub(super) struct Staged {
    pub media: BTreeMap<String, Media>,
    created: Vec<PathBuf>,
    homes: BTreeMap<String, Option<AssetStamp>>,
}
#[derive(Default)]
pub(super) struct Verification(BTreeMap<String, MatchedMedia>);
struct MatchedMedia {
    home: AssetStamp,
    source: AssetStamp,
}
impl Staged {
    pub fn load(root: &Path, source: BTreeMap<String, Media>) -> Result<Self, Error> {
        Self::load_verified(root, source, Verification::default())
    }
    pub fn load_verified(
        root: &Path,
        source: BTreeMap<String, Media>,
        verified: Verification,
    ) -> Result<Self, Error> {
        let mut staged = Self {
            media: BTreeMap::new(),
            created: Vec::new(),
            homes: BTreeMap::new(),
        };
        for (id, mut media) in source {
            let temporary = root.join("Media").join(format!(".{id}-{}.tmp", new_id()?));
            let linked = fs::hard_link(&media.path, &temporary).is_ok();
            if !linked {
                staged.created.push(temporary.clone());
                fs::copy(&media.path, &temporary)?;
                fs::OpenOptions::new()
                    .write(true)
                    .open(&temporary)?
                    .sync_all()?;
            } else {
                staged.created.push(temporary.clone());
            }
            media.path = temporary;
            let home = root.join("Media").join(format!("{id}.wav"));
            let existing = match fs::symlink_metadata(&home) {
                Ok(_) => {
                    let before = stamp(&home)?;
                    let source_before = stamp(&media.path)?;
                    // A successful link retains the exact verified inode. A
                    // copy, replacement or modification must be checked again.
                    let reused = linked
                        && (before.same_file(&source_before)
                            || verified.0.get(&id).is_some_and(|value| {
                                value.home == before && value.source == source_before
                            }));
                    if !reused
                        && (hash_file(&home)? != hash_file(&media.path)?
                            || stamp(&home)? != before
                            || stamp(&media.path)? != source_before)
                    {
                        return Err(protocol("An asset ID refers to different custody audio"));
                    }
                    Some(before)
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.into()),
            };
            staged.homes.insert(id.clone(), existing);
            staged.media.insert(id, media);
        }
        Ok(staged)
    }
    /// Called by the serialized authority owner (or exclusively during startup).
    /// Concurrent workers only own unique temporary links, never canonical files.
    pub fn commit(mut self, root: &Path) -> Result<BTreeMap<String, Media>, Error> {
        let mut published = Self {
            media: BTreeMap::new(),
            created: Vec::new(),
            homes: BTreeMap::new(),
        };
        for (id, media) in &mut self.media {
            let home = root.join("Media").join(format!("{id}.wav"));
            if let Some(expected) = &self.homes[id] {
                if stamp(&home)? != *expected {
                    return Err(protocol("Custody audio changed while opening the project"));
                }
            } else {
                fs::hard_link(&media.path, &home).map_err(|error| {
                    if error.kind() == std::io::ErrorKind::AlreadyExists {
                        Error::Conflict(
                            "Custody audio changed while opening; retry the project".into(),
                        )
                    } else {
                        error.into()
                    }
                })?;
                published.created.push(home.clone());
            }
            media.path = home;
        }
        published.created.clear();
        Ok(std::mem::take(&mut self.media))
    }
}
impl Drop for Staged {
    fn drop(&mut self) {
        for path in &self.created {
            let _ = fs::remove_file(path);
        }
    }
}

/// Gives any asset whose ID already names *different* custody audio a fresh
/// ID (in the media map and every clip, take and clip-history reference),
/// so opening the project succeeds instead of failing on the clash.
pub(super) fn rekey_conflicts(
    root: &Path,
    project: &mut soundroom_engine::project::Project,
) -> Result<(usize, Verification), Error> {
    let mut renames = BTreeMap::new();
    let mut verified = Verification::default();
    for (id, media) in &project.media {
        let home = root.join("Media").join(format!("{id}.wav"));
        if home.exists() {
            let (home_before, source_before) = (stamp(&home)?, stamp(&media.path)?);
            let equal = home_before.same_file(&source_before)
                || hash_file(&home)? == hash_file(&media.path)?;
            if stamp(&home)? != home_before || stamp(&media.path)? != source_before {
                return Err(protocol("Audio changed while checking project custody"));
            }
            if equal {
                verified.0.insert(
                    id.clone(),
                    MatchedMedia {
                        home: home_before,
                        source: source_before,
                    },
                );
            } else {
                renames.insert(id.clone(), new_id()?);
            }
        }
    }
    for (old, new) in &renames {
        if let Some(media) = project.media.remove(old) {
            project.media.insert(new.clone(), media);
        }
    }
    let rename = |id: &mut String| {
        if let Some(new) = renames.get(&id.to_uppercase()) {
            *id = new.clone();
        }
    };
    for track in &mut project.session.tracks {
        for clip in track.clips.iter_mut().chain(
            track
                .recording_takes
                .iter_mut()
                .flat_map(|t| t.clips.iter_mut()),
        ) {
            rename(&mut clip.asset_id);
            for state in &mut clip.previous_states {
                rename(&mut state.asset_id);
            }
        }
    }
    Ok((renames.len(), verified))
}

pub(super) fn remove(root: &Path, path: &Path) -> Result<(), Error> {
    if path.parent().map(fs::canonicalize).transpose()?
        != Some(fs::canonicalize(root.join("Media"))?)
    {
        return Err(protocol(
            "Refusing to delete audio outside server media custody",
        ));
    }
    fs::remove_file(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::private_directory;
    use soundroom_engine::wave::{WaveReader, WaveWriter};
    #[test]
    fn cancelled_or_conflicting_load_never_removes_another_loads_media() {
        let root = std::env::temp_dir().join(format!("soundroom-custody-{}", new_id().unwrap()));
        private_directory(&root).unwrap();
        private_directory(&root.join("Media")).unwrap();
        let source = root.join("source.wav");
        let mut writer = WaveWriter::create(&source, 128).unwrap();
        writer.write(&[0.1; 128], &[0.1; 128]).unwrap();
        writer.finish().unwrap();
        let id = new_id().unwrap();
        let media = Media {
            path: source.clone(),
            info: WaveReader::open(&source).unwrap().info,
        };
        let first =
            Staged::load(&root, [(id.clone(), media.clone())].into_iter().collect()).unwrap();
        let second =
            Staged::load(&root, [(id.clone(), media.clone())].into_iter().collect()).unwrap();
        let committed = first.commit(&root).unwrap();
        assert!(second.commit(&root).is_err());
        let home = &committed[&id].path;
        assert_eq!(fs::read(home).unwrap(), fs::read(&source).unwrap());
        // A project reusing the UUID for different audio cannot overwrite custody.
        let other = root.join("other.wav");
        let mut writer = WaveWriter::create(&other, 128).unwrap();
        writer.write(&[0.9; 128], &[0.9; 128]).unwrap();
        writer.finish().unwrap();
        assert!(Staged::load(
            &root,
            [(
                id.clone(),
                Media {
                    path: other.clone(),
                    info: media.info
                }
            )]
            .into_iter()
            .collect()
        )
        .is_err());
        assert_eq!(fs::read(home).unwrap(), fs::read(&source).unwrap());
        assert!(remove(&root, &source).is_err());
        assert!(source.exists());
        remove(&root, home).unwrap();
        assert!(source.exists());
        assert_eq!(fs::read_dir(root.join("Media")).unwrap().count(), 0);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn opening_a_project_rekeys_ids_that_name_different_custody_audio() {
        let root = std::env::temp_dir().join(format!("soundroom-rekey-{}", new_id().unwrap()));
        private_directory(&root).unwrap();
        private_directory(&root.join("Media")).unwrap();
        let wav = |name: &str, value: f32| {
            let path = root.join(name);
            let mut writer = WaveWriter::create(&path, 64).unwrap();
            writer.write(&[value; 64], &[value; 64]).unwrap();
            writer.finish().unwrap();
            path
        };
        let (same, clash) = (new_id().unwrap(), new_id().unwrap());
        fs::copy(
            wav("a.wav", 0.1),
            root.join("Media").join(format!("{same}.wav")),
        )
        .unwrap();
        fs::copy(
            wav("b.wav", 0.2),
            root.join("Media").join(format!("{clash}.wav")),
        )
        .unwrap();
        let media = |path: PathBuf| Media {
            info: WaveReader::open(&path).unwrap().info,
            path,
        };
        let mut session = soundroom_engine::model::Session::default();
        let mut track = soundroom_engine::model::Track::default();
        for id in [&same, &clash] {
            track.clips.push(serde_json::from_value(serde_json::json!({"id": new_id().unwrap(), "assetID": id.to_lowercase(), "name": "Clip", "start": 0, "offset": 0, "length": 64})).unwrap());
        }
        session.tracks.push(track);
        let mut project = soundroom_engine::project::Project {
            session,
            media: [
                (same.clone(), media(root.join("a.wav"))),
                (clash.clone(), media(wav("c.wav", 0.9))),
            ]
            .into_iter()
            .collect(),
        };
        assert_eq!(rekey_conflicts(&root, &mut project).unwrap().0, 1);
        let ids: Vec<_> = project.session.tracks[0]
            .clips
            .iter()
            .map(|c| c.asset_id.clone())
            .collect();
        assert_eq!(ids[0], same.to_lowercase(), "identical audio keeps its ID");
        assert_ne!(ids[1].to_uppercase(), clash);
        assert!(project.media.contains_key(&ids[1]) && !project.media.contains_key(&clash));
        assert!(Staged::load(&root, project.media.clone()).is_ok());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn unchanged_reopen_reuses_verified_hashes_and_rejects_later_mutation() {
        let root = std::env::temp_dir().join(format!("soundroom-verified-{}", new_id().unwrap()));
        private_directory(&root).unwrap();
        private_directory(&root.join("Media")).unwrap();
        let source = root.join("source.wav");
        let mut writer = WaveWriter::create(&source, 128).unwrap();
        writer.write(&[0.25; 128], &[0.25; 128]).unwrap();
        writer.finish().unwrap();
        let id = new_id().unwrap();
        let home = root.join("Media").join(format!("{id}.wav"));
        fs::copy(&source, &home).unwrap();
        let media = Media {
            info: WaveReader::open(&source).unwrap().info,
            path: source.clone(),
        };
        let mut project = soundroom_engine::project::Project {
            session: soundroom_engine::model::Session::default(),
            media: [(id.clone(), media.clone())].into_iter().collect(),
        };
        super::super::TEST_HASHED_BYTES.with(|count| count.set(0));
        let (renamed, verified) = rekey_conflicts(&root, &mut project).unwrap();
        assert_eq!(renamed, 0);
        let expected = fs::metadata(&source).unwrap().len() * 2;
        assert_eq!(
            super::super::TEST_HASHED_BYTES.with(|count| count.get()),
            expected
        );
        let staged = Staged::load_verified(&root, project.media.clone(), verified).unwrap();
        assert_eq!(
            super::super::TEST_HASHED_BYTES.with(|count| count.get()),
            expected,
            "Staging must not hash the same two files again"
        );
        staged.commit(&root).unwrap();
        let (_, verified) = rekey_conflicts(&root, &mut project).unwrap();
        // Replace the verified source after comparison, leaving custody intact.
        fs::remove_file(&source).unwrap();
        let mut writer = WaveWriter::create(&source, 129).unwrap();
        writer.write(&[0.9; 129], &[0.9; 129]).unwrap();
        writer.finish().unwrap();
        assert!(Staged::load_verified(&root, project.media, verified).is_err());
        assert_eq!(WaveReader::open(&home).unwrap().info.frames, 128);
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn linked_project_reopen_avoids_reading_the_entire_audio_twice() {
        let root =
            std::env::temp_dir().join(format!("soundroom-linked-reopen-{}", new_id().unwrap()));
        private_directory(&root).unwrap();
        private_directory(&root.join("Media")).unwrap();
        let source = root.join("source.wav");
        let frames = 4 * 1024 * 1024;
        let mut writer = WaveWriter::create(&source, frames).unwrap();
        for _ in 0..frames / 8192 {
            writer.write(&[0.25; 8192], &[0.125; 8192]).unwrap();
        }
        writer.finish().unwrap();
        let id = new_id().unwrap();
        let home = root.join("Media").join(format!("{id}.wav"));
        fs::hard_link(&source, &home).unwrap();
        let baseline = std::time::Instant::now();
        assert_eq!(hash_file(&source).unwrap(), hash_file(&home).unwrap());
        let baseline = baseline.elapsed();
        super::super::TEST_HASHED_BYTES.with(|count| count.set(0));
        let mut project = soundroom_engine::project::Project {
            session: Default::default(),
            media: [(
                id,
                Media {
                    info: WaveReader::open(&source).unwrap().info,
                    path: source.clone(),
                },
            )]
            .into_iter()
            .collect(),
        };
        let start = std::time::Instant::now();
        let (renamed, verified) = rekey_conflicts(&root, &mut project).unwrap();
        assert_eq!(renamed, 0);
        Staged::load_verified(&root, project.media, verified)
            .unwrap()
            .commit(&root)
            .unwrap();
        let elapsed = start.elapsed();
        assert_eq!(super::super::TEST_HASHED_BYTES.with(|count| count.get()), 0);
        println!("Linked 32 MiB project verification: previous_two_hashes_ms={:.3} inode_verified_reopen_ms={:.3} previous_audio_bytes_read={} current_audio_bytes_read=0", baseline.as_secs_f64()*1000., elapsed.as_secs_f64()*1000., fs::metadata(&source).unwrap().len()*2);
        fs::remove_dir_all(root).unwrap();
    }
}
