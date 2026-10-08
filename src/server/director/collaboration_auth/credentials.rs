use psf_guard_director_meta::{collaboration_connection::ConnectionBinding, Uuid};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};
pub(super) mod private_file;
const MAX_FILE: u64 = 2 * 1024 * 1024;

// No Debug: neither errors nor tracing should expose stored secrets.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    id: Uuid,
    rig: Uuid,
    base_url: String,
    agent: String,
    token: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    version: u32,
    entries: Vec<Entry>,
}

pub(super) fn path(registry: &Path) -> PathBuf {
    if registry.file_name().and_then(|n| n.to_str()) == Some("config.json") {
        registry.with_file_name("collaboration-credentials.json")
    } else {
        registry.with_file_name(format!(
            "{}.collaboration-credentials.json",
            registry
                .file_stem()
                .and_then(|n| n.to_str())
                .unwrap_or("config")
        ))
    }
}
fn denied() -> std::io::Error {
    std::io::Error::other(
        "Collaboration credentials unavailable; check config file ownership and permissions",
    )
}
fn load(path: &Path) -> std::io::Result<Document> {
    let file = match private_file::open(path, false) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Document {
                version: 1,
                entries: vec![],
            })
        }
        Err(_) => return Err(denied()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_FILE + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE {
        return Err(denied());
    }
    let doc: Document = serde_json::from_slice(&bytes).map_err(|_| denied())?;
    let mut ids = std::collections::BTreeSet::new();
    if doc.version != 1
        || doc.entries.len() > 256
        || doc.entries.iter().any(|e| {
            !ids.insert(e.id)
                || !psf_guard_director_interop::auth::valid_token(&e.token)
                || psf_guard_director_interop::astrocollab::Source::new(&e.base_url, &e.agent, true)
                    .is_err()
        })
    {
        return Err(denied());
    }
    Ok(doc)
}
fn parent(path: &Path) -> std::io::Result<&Path> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    private_file::check_parent(parent)?;
    Ok(parent)
}
fn lock(path: &Path) -> std::io::Result<File> {
    parent(path)?;
    let lock_path = path.with_extension("lock");
    let file = match private_file::open(&lock_path, true) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            private_file::open(&lock_path, false)?
        }
        Err(e) => return Err(e),
    };
    file.try_lock().map_err(|_| denied())?;
    Ok(file)
}
pub(super) fn available(path: &Path) -> std::io::Result<()> {
    let _lock = lock(path)?;
    load(path)?;
    // Probe the actual volume without changing an existing token file.
    let probe = Temp::new(parent(path)?)?;
    probe.file.sync_all()
}
pub(super) fn read(path: &Path, binding: &ConnectionBinding) -> std::io::Result<Option<String>> {
    parent(path)?;
    let doc = load(path)?;
    match doc.entries.into_iter().find(|e| e.id == binding.id) {
        None => Ok(None),
        Some(e)
            if e.rig == binding.rig_id
                && e.base_url == binding.base_url
                && Some(&e.agent) == binding.agent_id.as_ref() =>
        {
            Ok(Some(e.token))
        }
        Some(_) => Err(denied()),
    }
}
pub(super) fn write(
    path: &Path,
    binding: &ConnectionBinding,
    token: Option<&str>,
) -> std::io::Result<()> {
    let _lock = lock(path)?;
    let mut doc = load(path)?;
    doc.entries.retain(|e| e.id != binding.id);
    if let Some(token) = token {
        if !psf_guard_director_interop::auth::valid_token(token) {
            return Err(denied());
        }
        doc.entries.push(Entry {
            id: binding.id,
            rig: binding.rig_id,
            base_url: binding.base_url.clone(),
            agent: binding.agent_id.clone().ok_or_else(denied)?,
            token: token.into(),
        });
    }
    let bytes = serde_json::to_vec_pretty(&doc).map_err(|_| denied())?;
    if doc.entries.len() > 256 || bytes.len() as u64 > MAX_FILE {
        return Err(denied());
    }
    let mut temp = Temp::new(parent(path)?)?;
    temp.file.write_all(&bytes)?;
    temp.file.sync_all()?;
    fs::rename(&temp.path, path)?;
    #[cfg(unix)]
    File::open(parent(path)?)?.sync_all()?;
    if let Some(token) = token
        && read(path, binding)?.as_deref() != Some(token)
    {
        return Err(denied());
    }
    Ok(())
}
struct Temp {
    path: PathBuf,
    file: File,
}
impl Temp {
    fn new(parent: &Path) -> std::io::Result<Self> {
        let path = parent.join(format!(".collaboration-{}.tmp", Uuid::new_v4()));
        Ok(Self {
            file: private_file::open(&path, true)?,
            path,
        })
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn binding() -> ConnectionBinding {
        ConnectionBinding {
            id: Uuid::new_v4(),
            rig_id: Uuid::new_v4(),
            base_url: "https://example.com/".into(),
            name: "Rig".into(),
            allow_loopback_http: false,
            agent_id: Some("000000000001".into()),
            state: psf_guard_director_meta::collaboration_connection::ConnectionState::Registered,
            settings: None,
            background: None,
        }
    }
    #[test]
    fn isolated_paths_roundtrip_delete_and_wrong_binding() {
        let dir = tempfile::tempdir().unwrap();
        private_file::private_test_directory(dir.path()).unwrap();
        let p = path(&dir.path().join("isolated.json"));
        assert_eq!(
            p.file_name().unwrap(),
            "isolated.collaboration-credentials.json"
        );
        let b = binding();
        available(&p).unwrap();
        assert_eq!(read(&p, &b).unwrap(), None);
        write(&p, &b, Some("opaque-test-token")).unwrap();
        assert_eq!(read(&p, &b).unwrap().as_deref(), Some("opaque-test-token"));
        let mut wrong = b.clone();
        wrong.agent_id = Some("000000000002".into());
        assert!(read(&p, &wrong).is_err());
        let other = binding();
        write(&p, &other, Some("other-token")).unwrap();
        write(&p, &b, None).unwrap();
        assert_eq!(read(&p, &other).unwrap().as_deref(), Some("other-token"));
        fs::remove_file(&p).unwrap();
        assert_eq!(read(&p, &b).unwrap(), None);
        assert!(b.agent_id.is_some());
    }
    #[cfg(unix)]
    #[test]
    fn private_from_creation_and_refuses_unsafe_files() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        private_file::private_test_directory(dir.path()).unwrap();
        let p = path(&dir.path().join("config.json"));
        let b = binding();
        write(&p, &b, Some("token")).unwrap();
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::set_permissions(&p, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read(&p, &b).is_err());
        assert!(write(&p, &b, None).is_err());
        fs::remove_file(&p).unwrap();
        let target = dir.path().join("target");
        fs::write(&target, "untouched").unwrap();
        symlink(&target, &p).unwrap();
        assert!(read(&p, &b).is_err());
        assert!(write(&p, &b, None).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "untouched");
    }
}
