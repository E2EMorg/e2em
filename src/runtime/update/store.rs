//! Private, atomic update state. Paths are derived from validated versions.
use super::{Candidate, State};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct Store {
    pub root: PathBuf,
}
impl Store {
    pub fn open(root: &Path) -> io::Result<Self> {
        // Never broaden permissions on an existing directory. On Windows the
        // grants directory already has the setup tool's protected owner DACL.
        let parent = root
            .parent()
            .ok_or_else(|| io::Error::other("missing update parent"))?;
        check_directory(parent)?;
        if !root.exists() {
            let builder = fs::DirBuilder::new();
            #[cfg(unix)]
            let mut builder = builder;
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(root)?;
        }
        check_directory(root)?;
        Ok(Self {
            root: root.to_owned(),
        })
    }
    pub fn read<T: DeserializeOwned>(&self, name: &str) -> io::Result<T> {
        let path = self.root.join(name);
        #[cfg(windows)]
        let bytes = e2em_platform::read_private_file(&path)?;
        #[cfg(not(windows))]
        let bytes = {
            let m = fs::symlink_metadata(&path)?;
            if !m.is_file() || m.len() > 65_536 {
                return Err(io::Error::other("invalid update state file"));
            }
            check_owned(&m)?;
            let mut bytes = Vec::new();
            fs::File::open(path)?.take(65_537).read_to_end(&mut bytes)?;
            if bytes.len() > 65_536 {
                return Err(io::Error::other("oversized update state"));
            }
            bytes
        };
        serde_json::from_slice(&bytes).map_err(io::Error::other)
    }
    pub fn write(&self, name: &str, value: &impl Serialize) -> io::Result<()> {
        let mut tmp = tempfile::Builder::new()
            .prefix(".e2em-update-")
            .tempfile_in(&self.root)?;
        serde_json::to_writer_pretty(tmp.as_file_mut(), value).map_err(io::Error::other)?;
        tmp.write_all(b"\n")?;
        tmp.as_file().sync_all()?;
        tmp.persist(self.root.join(name))
            .map_err(io::Error::other)?;
        #[cfg(unix)]
        fs::File::open(&self.root)?.sync_all()?;
        Ok(())
    }
    pub fn state(&self) -> io::Result<State> {
        match self.read("state.json") {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(State::default()),
            result => result,
        }
    }
    pub fn binary(&self, candidate: &Candidate) -> io::Result<PathBuf> {
        let version = semver::Version::parse(&candidate.version).map_err(io::Error::other)?;
        if version.to_string() != candidate.version || !version.build.is_empty() {
            return Err(io::Error::other("noncanonical update version"));
        }
        Ok(self.root.join(format!(
            "e2emd-{}{}",
            candidate.version,
            std::env::consts::EXE_SUFFIX
        )))
    }
    pub fn verify(&self, candidate: &Candidate) -> io::Result<PathBuf> {
        let path = self.binary(candidate)?;
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file()
            || metadata.len() != candidate.size
            || candidate.size > super::MAX_BINARY
        {
            return Err(io::Error::other("invalid cached update binary"));
        }
        check_owned(&metadata)?;
        if digest(&path)? != candidate.sha256 {
            return Err(io::Error::other("cached update checksum mismatch"));
        }
        Ok(path)
    }
    pub fn cleanup(&self, state: &State) -> io::Result<()> {
        let keep: Vec<_> = [&state.active, &state.previous, &state.pending]
            .into_iter()
            .flatten()
            .map(|c| self.binary(c))
            .collect::<io::Result<_>>()?;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".e2em-update-") && entry.file_type()?.is_file() {
                let _ = fs::remove_file(entry.path());
                continue;
            }
            // Restrict cleanup to generated binary names; no recursive removal.
            if let Some(version) = name
                .strip_prefix("e2emd-")
                .and_then(|s| s.strip_suffix(std::env::consts::EXE_SUFFIX))
                && semver::Version::parse(version).is_ok()
                && entry.file_type()?.is_file()
                && !keep.contains(&entry.path())
            {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(())
    }
}
pub fn digest(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = [0; 65_536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hash.update(&buf[..n]);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn check_directory(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(io::Error::other(
            "update directory must be a regular directory",
        ));
    }
    check_owned(&metadata)?;
    #[cfg(windows)]
    {
        // Validate the inherited DACL before creating executable payloads.
        let probe = tempfile::NamedTempFile::new_in(path)?;
        e2em_platform::read_private_file(probe.path())?;
    }
    Ok(())
}
fn check_owned(metadata: &fs::Metadata) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != rustix::process::getuid().as_raw() || metadata.mode() & 0o077 != 0 {
            return Err(io::Error::other("update path must be owned and private"));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(io::Error::other("update reparse points are forbidden"));
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    fn fixture() -> (tempfile::TempDir, Store, Candidate) {
        let root = tempfile::tempdir().unwrap();
        fs::set_permissions(root.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let store = Store::open(&root.path().join("updates")).unwrap();
        let candidate = Candidate {
            version: "0.2.0".into(),
            sha256: format!("{:x}", Sha256::digest(b"binary")),
            size: 6,
        };
        let path = store.binary(&candidate).unwrap();
        fs::write(&path, b"binary").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        (root, store, candidate)
    }
    #[test]
    fn cached_payloads_are_reverified_and_symlinks_are_rejected() {
        let (_root, store, candidate) = fixture();
        let path = store.verify(&candidate).unwrap();
        fs::write(&path, b"broken").unwrap();
        assert!(store.verify(&candidate).is_err());
        fs::remove_file(&path).unwrap();
        symlink("/bin/sh", path).unwrap();
        assert!(store.verify(&candidate).is_err());
        let bad = Candidate {
            version: "../escape".into(),
            ..candidate
        };
        assert!(store.binary(&bad).is_err());
    }
    #[test]
    fn private_state_is_atomic_and_cleanup_preserves_other_files() {
        let (_root, store, candidate) = fixture();
        let state = State {
            active: Some(candidate.clone()),
            ..State::default()
        };
        store.write("state.json", &state).unwrap();
        assert_eq!(store.state().unwrap().active, Some(candidate));
        assert_eq!(
            fs::metadata(store.root.join("state.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::write(store.root.join("e2emd-0.1.9"), "old").unwrap();
        fs::write(store.root.join("owner-note"), "keep").unwrap();
        store.cleanup(&state).unwrap();
        assert!(store.root.join("e2emd-0.2.0").exists());
        assert!(!store.root.join("e2emd-0.1.9").exists());
        assert!(store.root.join("owner-note").exists());
    }
    #[test]
    fn shared_or_symlinked_directories_are_rejected() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("updates");
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(Store::open(&directory).is_err());
        fs::remove_dir(&directory).unwrap();
        symlink(root.path(), &directory).unwrap();
        assert!(Store::open(&directory).is_err());
    }
}
