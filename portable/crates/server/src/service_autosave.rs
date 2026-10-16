//! Local-host continuity: the current session is autosaved off the control lock
//! (hard-linked media plus session JSON) and the newest autosave reopens on start.
use super::{engine, new_id, private_directory, Error, Service, Shared};
use soundroom_engine::project::Project;
use std::{
    fs,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};
const POINTER: &str = "current";
#[derive(Default)]
pub(super) struct State {
    revision: Option<u64>,
}
impl Service {
    /// Synchronous save of the current session (shutdown path).
    pub(super) fn autosave_now(&self) -> Result<(), Error> {
        save(
            &self.root,
            &Project {
                session: self.authority.session.clone(),
                media: self.media.clone(),
            },
            self.authority.revision,
            &self.autosave,
        )
    }
    /// Saves at most once per second after the authored revision changes.
    pub fn start_autosave(shared: &Shared) {
        let weak = Arc::downgrade(shared);
        std::thread::spawn(move || {
            let mut saved = None;
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let Some(shared) = weak.upgrade() else { return };
                let captured = match shared.try_lock() {
                    Ok(state) if saved != Some(state.authority.revision) => Some((
                        state.authority.revision,
                        state.root.clone(),
                        state.autosave.clone(),
                        Project {
                            session: state.authority.session.clone(),
                            media: state.media.clone(),
                        },
                    )),
                    _ => None,
                };
                drop(shared);
                if let Some((revision, root, publication, project)) = captured {
                    match save(&root, &project, revision, &publication) {
                        Ok(()) => saved = Some(revision),
                        Err(error) => eprintln!("Soundroom autosave failed: {error}"),
                    }
                }
            }
        });
    }
    /// Opens the newest autosave (if any) with its media re-homed under Media/,
    /// so pruning older autosaves never strands a live source path.
    pub fn open_with_autosave(root: &Path) -> Result<Shared, Error> {
        let latest = fs::read_to_string(root.join("Autosave").join(POINTER))
            .ok()
            .map(|name| root.join("Autosave").join(name.trim()))
            .filter(|path| path.is_dir());
        Self::open(root, latest.as_deref())
    }
}
fn save(
    root: &Path,
    project: &Project,
    revision: u64,
    publication: &Mutex<State>,
) -> Result<(), Error> {
    // Background capture drops the authority lock before taking this mutex.
    // Synchronous saves hold authority then publication; no reverse acquisition
    // exists, and an older captured revision cannot publish after a newer save.
    let mut publication = publication
        .lock()
        .map_err(|_| Error::Protocol("Recovery publication is unavailable".into()))?;
    if publication.revision.is_some_and(|saved| revision < saved) {
        return Ok(());
    }
    let dir = root.join("Autosave");
    private_directory(&dir)?;
    let name = format!("{}.soundroom", new_id()?);
    project.save_new(&dir.join(&name)).map_err(engine)?;
    let temporary = dir.join(format!(".current-{}.tmp", new_id()?));
    let published = (|| {
        super::super::registry::write_new_private(&temporary, name.as_bytes())?;
        fs::rename(&temporary, dir.join(POINTER))?;
        #[cfg(unix)]
        fs::File::open(&dir)?.sync_all()?;
        Ok::<_, Error>(())
    })();
    let _ = fs::remove_file(&temporary);
    published?;
    publication.revision = Some(revision);
    for entry in fs::read_dir(&dir)?.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        if file.ends_with(".soundroom") && file != name {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_captured_revision_never_replaces_or_prunes_newer_recovery() {
        let root = std::env::temp_dir().join(format!("soundroom-autosave-{}", new_id().unwrap()));
        private_directory(&root).unwrap();
        let state = Mutex::new(State::default());
        let mut project = Project {
            session: Default::default(),
            media: Default::default(),
        };
        project.session.name = "New".into();
        save(&root, &project, 2, &state).unwrap();
        let name = fs::read_to_string(root.join("Autosave/current")).unwrap();
        project.session.name = "Old".into();
        save(&root, &project, 1, &state).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("Autosave/current")).unwrap(),
            name
        );
        assert_eq!(
            Project::open(&root.join("Autosave").join(name))
                .unwrap()
                .session
                .name,
            "New"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
