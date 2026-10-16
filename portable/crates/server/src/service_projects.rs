//! Worker-side project download for a GUI saving back to its own filesystem.
use super::*;

pub(super) fn package(
    shared: &Shared,
    peer: &Identity,
    project: &Project,
    cancel: &AtomicBool,
) -> Result<Option<Value>, Error> {
    let root = shared
        .lock()
        .map_err(|_| protocol("Authority is unavailable"))?
        .root
        .clone();
    let directory = root
        .join("Artifacts")
        .join(format!("{}.project-stage", new_id()?));
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    std::fs::create_dir(&directory)?;
    let _cleanup = Cleanup(directory.clone());
    let package = directory.join("Session.soundroom");
    project.save_new(&package).map_err(engine)?;
    if cancel.load(Ordering::Acquire) {
        return Err(protocol("Save cancelled"));
    }
    let archive = directory.join("Session.soundroom.zip");
    let status = std::process::Command::new("/usr/bin/ditto")
        .args([
            "-c",
            "-k",
            "--norsrc",
            "--keepParent",
            "--zlibCompressionLevel",
            "0",
        ])
        .arg(&package)
        .arg(&archive)
        .status()?;
    if !status.success() {
        return Err(protocol(
            "The server could not prepare the project for saving",
        ));
    }
    if cancel.load(Ordering::Acquire) {
        return Err(protocol("Save cancelled"));
    }
    let (bytes, hash) = hash_file(&archive)?;
    if bytes > 2 * 1024 * 1024 * 1024 {
        return Err(protocol(
            "The project download exceeds the current 2 GiB transfer limit",
        ));
    }
    let id = new_id()?;
    let destination = root.join("Artifacts").join(format!("{id}.soundroom.zip"));
    let mut state = shared
        .lock()
        .map_err(|_| protocol("Authority is unavailable"))?;
    if cancel.load(Ordering::Acquire) || !state.registry.authorizes(&peer.credential, &peer.role) {
        return Err(protocol("Save was cancelled or its credential was revoked"));
    }
    if state.artifacts.len() >= 32
        || state.artifacts.values().map(|a| a.bytes).sum::<u64>() + bytes > 4 * 1024 * 1024 * 1024
    {
        return Err(Error::QueueFull);
    }
    std::fs::rename(archive, &destination)?;
    state.artifacts.insert(
        id.clone(),
        Artifact {
            owner: peer.credential.clone(),
            runtime: Some(peer.runtime.clone()),
            path: destination,
            bytes,
            expires: Instant::now() + Duration::from_secs(3600),
        },
    );
    Ok(Some(
        json!({"id":id,"name":"Session.soundroom.zip","bytes":bytes,"sha256":hash}),
    ))
}
