//! Downloads of ONNX Runtime and the noise model. Both start only when a
//! person asks: a button in Settings on the server, a command on a worker.
//!
//! The model comes through Seiza, which verifies each file against its
//! manifest. Seiza does not fetch ONNX Runtime, so PSF Guard downloads
//! Microsoft's release archive for this platform, checks it against the
//! SHA-256 below, and unpacks the libraries into its own folder.

use std::io::Read;
use std::path::{Path, PathBuf};

use super::{Config, LIBRARY_FILE};

/// The ONNX Runtime release PSF Guard downloads. `seiza-model` pins
/// `ort =2.0.0-rc.15`, which opens ONNX Runtime 1.17 or later; Seiza
/// measured its model against this release.
pub const ONNXRUNTIME_VERSION: &str = "1.31.1";

const RELEASE_URL: &str = "https://github.com/microsoft/onnxruntime/releases/download";

/// One release archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Release {
    pub asset: &'static str,
    /// The archive's SHA-256, computed from the file Microsoft published.
    pub sha256: &'static str,
}

/// The archive for a platform, by Rust's names for it. `None` where
/// Microsoft publishes no CPU archive, such as a Mac with an Intel
/// processor: there a person installs ONNX Runtime and sets its path.
pub fn release_for(os: &str, arch: &str) -> Option<Release> {
    let (asset, sha256) = match (os, arch) {
        ("linux", "x86_64") => (
            "onnxruntime-linux-x64-1.31.1.tgz",
            "c849e36a1ef3e22d4ff217b9d1ce4a3cf5b9467b975610913ae959ad5b16bdc9",
        ),
        ("linux", "aarch64") => (
            "onnxruntime-linux-aarch64-1.31.1.tgz",
            "5317505e2da99be65f8e2775ba3c2afd6d2961fd78aba1bd8f56159f48820c78",
        ),
        ("macos", "aarch64") => (
            "onnxruntime-osx-arm64-1.31.1.tgz",
            "2e1344a1926ee50eb25b38153cb6992c3567515aef0e677a1a520a1798896d6f",
        ),
        ("windows", "x86_64") => (
            "onnxruntime-win-x64-1.31.1.zip",
            "5869e26bb25314375f2d5ef1877e3ea06b1543d6c710e6e4a1ec2f7177ab4b2c",
        ),
        ("windows", "aarch64") => (
            "onnxruntime-win-arm64-1.31.1.zip",
            "817f4129073f84f81cde3ff92b1a4e06d007691e7208deed08a8b6d6df143f7f",
        ),
        _ => return None,
    };
    Some(Release { asset, sha256 })
}

/// The archive for this computer.
pub fn release() -> Option<Release> {
    release_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// The message where PSF Guard has no archive to download.
pub const NO_RELEASE: &str = "PSF Guard has no ONNX Runtime download for this computer. \
     Install ONNX Runtime 1.17 or later and set the path of its library";

/// How far a download has got.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Step {
    pub message: String,
    pub bytes_done: Option<u64>,
    pub bytes_total: Option<u64>,
}

impl Step {
    fn say(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            ..Default::default()
        }
    }
}

/// What a download of ONNX Runtime ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeInstall {
    /// The library's path.
    pub library: PathBuf,
    /// The release was installed already, so nothing was downloaded.
    pub already_installed: bool,
}

/// The file that says which folder holds the ONNX Runtime in use. A new
/// download goes to a folder of its own and this file is switched to it, so
/// a library a process has open is never deleted from under it.
pub(super) const CURRENT: &str = "onnxruntime.current";
/// Starts the name of each folder a download unpacks into.
const INSTALL_PREFIX: &str = "onnxruntime-";
/// Starts the name of a folder a download is unpacking into.
const STAGING_PREFIX: &str = ".unpack-";
/// Ends the name of an archive being downloaded.
const PART_SUFFIX: &str = ".part";

/// The folder of the ONNX Runtime in use, when a download installed one.
pub(super) fn current_folder(folder: &Path) -> Option<PathBuf> {
    let name = std::fs::read_to_string(folder.join(CURRENT)).ok()?;
    let name = name.trim();
    // The file gives a folder beside it, never a path.
    (name.starts_with(INSTALL_PREFIX) && !name.contains(['/', '\\'])).then(|| folder.join(name))
}

/// The release a download installed in `folder`, when its library is there.
pub fn installed_version(folder: &Path) -> Option<String> {
    let current = current_folder(folder)?;
    current.join(LIBRARY_FILE).is_file().then_some(())?;
    Some(
        std::fs::read_to_string(current.join("VERSION"))
            .ok()?
            .trim()
            .to_string(),
    )
}

/// Remove what earlier downloads left: archives and unpack folders of a
/// download that was stopped or crashed, and installs no longer in use. A
/// folder that cannot be removed, such as one whose library a process
/// still has open on Windows, stays for the next time.
fn sweep(folder: &Path) {
    let current = current_folder(folder);
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(PART_SUFFIX) && path.is_file() {
            let _ = std::fs::remove_file(&path);
        } else if name.starts_with(STAGING_PREFIX)
            || (name.starts_with(INSTALL_PREFIX)
                && path.is_dir()
                && current.as_deref() != Some(path.as_path()))
        {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// Removes a download's archive and unpack folder when the download ends,
/// however it ends: finished, failed, or dropped by Stop part way. It also
/// tells an unpack still running on another thread to throw its work away.
struct Cleanup {
    archive: PathBuf,
    staging: PathBuf,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let _ = std::fs::remove_file(&self.archive);
        let _ = std::fs::remove_dir_all(&self.staging);
    }
}

/// Download this platform's ONNX Runtime into `config`'s folder. The
/// archive is verified before anything is unpacked, and the install appears
/// whole or not at all. When the release is installed already, nothing is
/// downloaded unless `force` asks for it again.
pub async fn download_runtime(
    config: &Config,
    force: bool,
    report: &(dyn Fn(Step) + Send + Sync),
) -> Result<RuntimeInstall, String> {
    let release = release().ok_or_else(|| NO_RELEASE.to_string())?;
    download_runtime_from(
        config,
        release,
        &format!("{RELEASE_URL}/v{ONNXRUNTIME_VERSION}/{}", release.asset),
        force,
        report,
    )
    .await
}

async fn download_runtime_from(
    config: &Config,
    release: Release,
    url: &str,
    force: bool,
    report: &(dyn Fn(Step) + Send + Sync),
) -> Result<RuntimeInstall, String> {
    let folder = config.folder.clone();
    std::fs::create_dir_all(&folder)
        .map_err(|error| format!("Creating {}: {error}", folder.display()))?;
    sweep(&folder);
    if !force
        && installed_version(&folder).as_deref() == Some(ONNXRUNTIME_VERSION)
        && let Some(current) = current_folder(&folder)
    {
        report(Step::say(format!(
            "ONNX Runtime {ONNXRUNTIME_VERSION} is already installed"
        )));
        return Ok(RuntimeInstall {
            library: current.join(LIBRARY_FILE),
            already_installed: true,
        });
    }
    let tag = unique_tag();
    let cleanup = Cleanup {
        archive: folder.join(format!(".{}{PART_SUFFIX}", release.asset)),
        staging: folder.join(format!("{STAGING_PREFIX}{tag}")),
        cancelled: std::sync::Arc::default(),
    };
    fetch(url, &cleanup.archive, release, report).await?;
    report(Step::say(format!(
        "Unpacking ONNX Runtime {ONNXRUNTIME_VERSION}…"
    )));
    let (archive, staging) = (cleanup.archive.clone(), cleanup.staging.clone());
    let cancelled = std::sync::Arc::clone(&cleanup.cancelled);
    let library = tokio::task::spawn_blocking(move || {
        install_archive(&archive, release.asset, &staging, &folder, &tag, &cancelled)
    })
    .await
    .map_err(|error| format!("Unpacking ONNX Runtime failed: {error}"))??;
    Ok(RuntimeInstall {
        library,
        already_installed: false,
    })
}

/// A name part no other download of this or another process has.
fn unique_tag() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{:x}-{:x}", std::process::id(), nanos)
}

/// Download `url` to `archive` and check it against the release's hash.
async fn fetch(
    url: &str,
    archive: &Path,
    release: Release,
    report: &(dyn Fn(Step) + Send + Sync),
) -> Result<(), String> {
    use crate::verified_download::{Expected, Failure};

    let message = format!("Downloading ONNX Runtime {ONNXRUNTIME_VERSION}…");
    let progress = |done: u64, total: Option<u64>| {
        report(Step {
            message: message.clone(),
            bytes_done: Some(done),
            bytes_total: total,
        })
    };
    crate::verified_download::fetch(
        url,
        archive,
        Expected {
            bytes: None,
            sha256: Some(release.sha256),
        },
        MAX_ARCHIVE_BYTES,
        &|| false,
        &progress,
    )
    .await
    .map_err(|failure| match failure {
        Failure::Transfer(error) => format!("Downloading {}: {error}", release.asset),
        Failure::Write(error) => format!("Writing {error}"),
        Failure::TooLarge { limit } => {
            format!(
                "{} is larger than {limit} bytes. Nothing was installed",
                release.asset
            )
        }
        Failure::NotOnRecord { sha256, .. } => format!(
            "{} does not match its SHA-256: expected {}, found {sha256}. Nothing was installed",
            release.asset, release.sha256
        ),
        Failure::Stopped => "Stopped".into(),
    })
}

/// No ONNX Runtime archive Microsoft publishes comes near this.
const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;

/// Unpack a verified archive into `staging`, move that to a folder of its
/// own in `folder`, and switch [`CURRENT`] to it. Returns the library's
/// path. The install in use before stays where it is until a later download
/// sweeps it. `cancelled` is read before the switch: a download that was
/// stopped while this ran installs nothing.
fn install_archive(
    archive: &Path,
    asset: &str,
    staging: &Path,
    folder: &Path,
    tag: &str,
    cancelled: &std::sync::atomic::AtomicBool,
) -> Result<PathBuf, String> {
    let stopped = || cancelled.load(std::sync::atomic::Ordering::SeqCst);
    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging)
        .map_err(|error| format!("Creating {}: {error}", staging.display()))?;
    let unpacked = if asset.ends_with(".zip") {
        unpack_zip(archive, staging)
    } else {
        unpack_tgz(archive, staging)
    };
    let discard = |reason: String| {
        let _ = std::fs::remove_dir_all(staging);
        reason
    };
    unpacked.map_err(|error| discard(format!("Unpacking {asset}: {error}")))?;
    if !staging.join(LIBRARY_FILE).is_file() {
        return Err(discard(format!("{asset} has no {LIBRARY_FILE}")));
    }
    std::fs::write(staging.join("VERSION"), ONNXRUNTIME_VERSION)
        .map_err(|error| discard(format!("Writing {}: {error}", staging.display())))?;
    if stopped() {
        return Err(discard("The download was stopped".into()));
    }
    let name = format!("{INSTALL_PREFIX}{ONNXRUNTIME_VERSION}-{tag}");
    let target = folder.join(&name);
    std::fs::rename(staging, &target).map_err(|error| {
        discard(format!(
            "Moving ONNX Runtime to {}: {error}",
            target.display()
        ))
    })?;
    // The switch is one rename, so a reader sees the old install or the
    // new one.
    let pointer = folder.join(format!("{CURRENT}.{tag}"));
    let switched = std::fs::write(&pointer, &name)
        .and_then(|()| std::fs::rename(&pointer, folder.join(CURRENT)));
    if let Err(error) = switched {
        let _ = std::fs::remove_file(&pointer);
        let _ = std::fs::remove_dir_all(&target);
        return Err(format!("Switching to {}: {error}", target.display()));
    }
    Ok(target.join(LIBRARY_FILE))
}

/// What an archive entry is to PSF Guard: a library in the archive's `lib`
/// folder, or the license texts beside it. Returns the file name to keep
/// it under; every kept file goes flat into one folder, so no entry can
/// write outside it.
fn kept_name(entry: &Path) -> Option<String> {
    // A path with `..`, a root or a drive in it is not one of ours.
    let parts: Vec<&str> = entry
        .components()
        .map(|part| match part {
            std::path::Component::Normal(name) => name.to_str(),
            _ => None,
        })
        .collect::<Option<_>>()?;
    match parts.as_slice() {
        [_, "lib", name] if is_library(name) => Some(name.to_string()),
        [_, name @ ("LICENSE" | "ThirdPartyNotices.txt")] => Some(name.to_string()),
        _ => None,
    }
}

/// Whether a file in `lib` is a shared library ONNX Runtime loads, on any
/// platform's naming.
fn is_library(name: &str) -> bool {
    let unix = name.starts_with("libonnxruntime")
        && (name.ends_with(".dylib") || name.ends_with(".so") || name.contains(".so."));
    let windows = name.starts_with("onnxruntime") && name.ends_with(".dll");
    unix || windows
}

fn unpack_tgz(archive: &Path, staging: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|error| error.to_string())?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    // A link is a name for another library in the same folder, such as
    // `libonnxruntime.so` for `libonnxruntime.so.1.31.1`.
    let mut links: Vec<(String, String)> = Vec::new();
    for entry in tar.entries().map_err(|error| error.to_string())? {
        let mut entry = entry.map_err(|error| error.to_string())?;
        let path = entry
            .path()
            .map_err(|error| error.to_string())?
            .into_owned();
        let Some(name) = kept_name(&path) else {
            continue;
        };
        let kind = entry.header().entry_type();
        if kind.is_symlink() || kind.is_hard_link() {
            let target = entry
                .link_name()
                .map_err(|error| error.to_string())?
                .and_then(|target| {
                    target
                        .file_name()
                        .and_then(|name| name.to_str())
                        .map(String::from)
                })
                .filter(|target| is_library(target))
                .ok_or_else(|| format!("{name} links outside the archive's lib folder"))?;
            links.push((name, target));
        } else if kind.is_file() {
            write_entry(&mut entry, &staging.join(&name), is_library(&name))?;
        }
    }
    place_links(staging, &links)?;
    match links.iter().find(|(name, _)| !staging.join(name).is_file()) {
        Some((name, target)) => Err(format!("{name} links to {target}, which is missing")),
        None => Ok(()),
    }
}

/// Give each linked library its second name in `staging`. The target is a
/// bare file name, so a link stays inside the folder and moves with it.
#[cfg(unix)]
fn place_links(staging: &Path, links: &[(String, String)]) -> Result<(), String> {
    for (name, target) in links {
        std::os::unix::fs::symlink(target, staging.join(name))
            .map_err(|error| format!("{name}: {error}"))?;
    }
    Ok(())
}

/// Give each linked library its second name in `staging`, as a copy: a
/// link needs a privilege on Windows. A link may name another link, so each
/// pass copies the ones whose target is now a file.
#[cfg(not(unix))]
fn place_links(staging: &Path, links: &[(String, String)]) -> Result<(), String> {
    let mut left: Vec<&(String, String)> = links.iter().collect();
    for _ in 0..4 {
        left.retain(|(name, target)| {
            let source = staging.join(target);
            !(source.is_file() && std::fs::copy(&source, staging.join(name)).is_ok())
        });
    }
    Ok(())
}

fn unpack_zip(archive: &Path, staging: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|error| error.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|error| error.to_string())?;
        if !entry.is_file() {
            continue;
        }
        let Some(name) = entry
            .name()
            .ok()
            .and_then(|name| kept_name(Path::new(name.as_ref())))
        else {
            continue;
        };
        let executable = is_library(&name);
        write_entry(&mut entry, &staging.join(name), executable)?;
    }
    Ok(())
}

fn write_entry(entry: &mut dyn Read, to: &Path, executable: bool) -> Result<(), String> {
    let mut file =
        std::fs::File::create(to).map_err(|error| format!("{}: {error}", to.display()))?;
    std::io::copy(entry, &mut file).map_err(|error| format!("{}: {error}", to.display()))?;
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(to, std::fs::Permissions::from_mode(0o755));
    }
    #[cfg(not(unix))]
    let _ = executable;
    Ok(())
}

/// Download the latest noise model Seiza publishes into `config`'s folder
/// and return its name. Seiza verifies every file. A model PSF Guard
/// downloaded before is removed, so one model is installed at a time.
pub async fn download_model(
    config: &Config,
    report: &(dyn Fn(Step) + Send + Sync),
) -> Result<String, String> {
    use seiza_download::DownloadEvent;

    // The manifest is read fresh, so "latest" means latest.
    let manager = seiza_download::ModelManager::builder()
        .policy(seiza_download::CachePolicy::ForceRefresh)
        .build()
        .map_err(|error| error.to_string())?;
    let store = seiza_model::ModelStore::new(config.models());
    let bundle = store
        .download(
            &manager,
            seiza_model::Published::Latest(seiza_model::Task::Denoise),
            |event| match event {
                DownloadEvent::FetchingManifest { .. } => {
                    report(Step::say("Checking which noise model is the latest…"))
                }
                DownloadEvent::DownloadStarted { name, bytes } => report(Step {
                    message: format!("Downloading {name}…"),
                    bytes_done: Some(0),
                    bytes_total: Some(bytes),
                }),
                DownloadEvent::DownloadProgress {
                    name,
                    downloaded,
                    total,
                    ..
                } => report(Step {
                    message: format!("Downloading {name}…"),
                    bytes_done: Some(downloaded),
                    bytes_total: Some(total),
                }),
                DownloadEvent::Verifying { name } => {
                    report(Step::say(format!("Verifying {name}…")))
                }
                DownloadEvent::Installing { name, .. } => {
                    report(Step::say(format!("Installing {name}…")))
                }
                _ => {}
            },
        )
        .await
        .map_err(|error| format!("The noise model could not be downloaded: {error}"))?;
    let name = bundle.card().name.clone();
    for other in store.list().map_err(|error| error.to_string())? {
        if other.card().task == seiza_model::Task::Denoise && other.card().name != name {
            let _ = store.remove(&other.card().name);
        }
    }
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn every_platform_with_an_archive_pins_a_whole_sha_256() {
        let platforms = [
            ("linux", "x86_64"),
            ("linux", "aarch64"),
            ("macos", "aarch64"),
            ("windows", "x86_64"),
            ("windows", "aarch64"),
        ];
        for (os, arch) in platforms {
            let release = release_for(os, arch).unwrap();
            assert!(release.asset.contains(ONNXRUNTIME_VERSION), "{os} {arch}");
            assert_eq!(release.sha256.len(), 64, "{os} {arch}");
            assert!(release.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
        }
        // Microsoft publishes no archive for an Intel Mac.
        assert_eq!(release_for("macos", "x86_64"), None);
        assert_eq!(release_for("freebsd", "x86_64"), None);
    }

    /// What a test archive's entry is.
    enum Entry {
        File(&'static [u8]),
        Link(&'static str),
    }
    use Entry::{File, Link};

    fn tgz(entries: &[(&str, Entry)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (path, entry) in entries {
            let mut header = tar::Header::new_gnu();
            match entry {
                File(body) => {
                    header.set_size(body.len() as u64);
                    header.set_mode(0o644);
                    header.set_entry_type(tar::EntryType::Regular);
                    header.set_cksum();
                    builder.append_data(&mut header, path, *body).unwrap();
                }
                Link(link) => {
                    header.set_size(0);
                    header.set_entry_type(tar::EntryType::Symlink);
                    builder.append_link(&mut header, path, link).unwrap();
                }
            }
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn a_linux_archive_unpacks_to_its_libraries_with_links_resolved() {
        let folder = tempfile::tempdir().unwrap();
        let archive = folder.path().join("ort.tgz");
        std::fs::write(
            &archive,
            tgz(&[
                ("onnxruntime-x/include/api.h", File(b"header")),
                (
                    "onnxruntime-x/lib/libonnxruntime.so",
                    Link("libonnxruntime.so.1"),
                ),
                (
                    "onnxruntime-x/lib/libonnxruntime.so.1",
                    Link("libonnxruntime.so.1.31.1"),
                ),
                (
                    "onnxruntime-x/lib/libonnxruntime.so.1.31.1",
                    File(b"the library"),
                ),
                (
                    "onnxruntime-x/lib/libonnxruntime_providers_shared.so",
                    File(b"shared"),
                ),
                ("onnxruntime-x/lib/pkgconfig/libonnxruntime.pc", File(b"pc")),
                ("onnxruntime-x/LICENSE", File(b"MIT")),
                ("onnxruntime-x/README.md", File(b"readme")),
            ]),
        )
        .unwrap();
        let staging = folder.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        unpack_tgz(&archive, &staging).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(&staging)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "LICENSE",
                "libonnxruntime.so",
                "libonnxruntime.so.1",
                "libonnxruntime.so.1.31.1",
                "libonnxruntime_providers_shared.so",
            ]
        );
        assert_eq!(
            std::fs::read(staging.join("libonnxruntime.so")).unwrap(),
            b"the library"
        );
    }

    #[test]
    fn an_archive_entry_never_lands_outside_the_folder() {
        for refused in [
            "../lib/libonnxruntime.so",
            "x/lib/../../libonnxruntime.so",
            "x/lib/sub/libonnxruntime.so",
            "/lib/libonnxruntime.so",
            "x/lib/notes.txt",
            "x/testdata/libcustom_op_library.dylib",
            "x/lib/libonnxruntime.1.31.1.dylib.dSYM/Contents/Info.plist",
        ] {
            assert_eq!(kept_name(Path::new(refused)), None, "{refused}");
        }
        assert_eq!(
            kept_name(Path::new("x/lib/onnxruntime.dll")).as_deref(),
            Some("onnxruntime.dll")
        );
        assert_eq!(
            kept_name(Path::new("x/lib/libonnxruntime.1.31.1.dylib")).as_deref(),
            Some("libonnxruntime.1.31.1.dylib")
        );
        // A link that points out of the lib folder is refused.
        let folder = tempfile::tempdir().unwrap();
        let archive = folder.path().join("ort.tgz");
        std::fs::write(
            &archive,
            tgz(&[("x/lib/libonnxruntime.so", Link("/etc/passwd"))]),
        )
        .unwrap();
        let refused = unpack_tgz(&archive, folder.path()).unwrap_err();
        assert!(refused.contains("links outside"), "{refused}");
    }

    #[test]
    fn a_windows_archive_unpacks_to_its_dlls() {
        let folder = tempfile::tempdir().unwrap();
        let archive = folder.path().join("ort.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        let options = zip::write::SimpleFileOptions::default();
        for (name, body) in [
            ("onnxruntime-w/lib/onnxruntime.dll", "the library"),
            (
                "onnxruntime-w/lib/onnxruntime_providers_shared.dll",
                "shared",
            ),
            ("onnxruntime-w/lib/onnxruntime.pdb", "symbols"),
            ("onnxruntime-w/lib/onnxruntime.lib", "import"),
            ("onnxruntime-w/ThirdPartyNotices.txt", "notices"),
        ] {
            zip.start_file(name, options).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
        let staging = folder.path().join("staging");
        std::fs::create_dir_all(&staging).unwrap();
        unpack_zip(&archive, &staging).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(&staging)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "ThirdPartyNotices.txt",
                "onnxruntime.dll",
                "onnxruntime_providers_shared.dll",
            ]
        );
    }

    fn library_entry() -> &'static str {
        if LIBRARY_FILE.ends_with(".dll") {
            "x/lib/onnxruntime.dll"
        } else if LIBRARY_FILE.ends_with(".dylib") {
            "x/lib/libonnxruntime.dylib"
        } else {
            "x/lib/libonnxruntime.so"
        }
    }

    /// Install a stand-in archive in `folder`, as a download does once the
    /// archive is verified.
    fn install(folder: &Path, tag: &str, body: &'static [u8]) -> Result<PathBuf, String> {
        // The test archive is a tgz on every platform; only the name the
        // platform loads differs.
        let archive = folder.join("ort.tgz");
        std::fs::write(&archive, tgz(&[(library_entry(), File(body))])).unwrap();
        install_archive(
            &archive,
            "ort.tgz",
            &folder.join(format!("{STAGING_PREFIX}{tag}")),
            folder,
            tag,
            &std::sync::atomic::AtomicBool::new(false),
        )
    }

    #[test]
    fn a_download_installs_beside_the_one_in_use_and_switches_to_it() {
        let folder = tempfile::tempdir().unwrap();
        let folder = folder.path();
        assert_eq!(installed_version(folder), None);
        let first = install(folder, "a", b"first").unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), b"first");
        assert_eq!(
            installed_version(folder).as_deref(),
            Some(ONNXRUNTIME_VERSION)
        );
        let in_use = current_folder(folder).unwrap();
        assert_eq!(first, in_use.join(LIBRARY_FILE));

        // A second download never touches the folder in use: a process may
        // have its library open, and Windows refuses to delete that.
        let second = install(folder, "b", b"second").unwrap();
        assert_ne!(second, first);
        assert_eq!(std::fs::read(&first).unwrap(), b"first");
        assert_eq!(current_folder(folder).unwrap().join(LIBRARY_FILE), second);
        // Nothing is left of the unpacking.
        assert!(!folder.join(format!("{STAGING_PREFIX}b")).exists());

        // An archive without the library installs nothing and switches
        // nothing.
        let archive = folder.join("bad.tgz");
        std::fs::write(&archive, tgz(&[("x/LICENSE", File(b"MIT"))])).unwrap();
        let refused = install_archive(
            &archive,
            "bad.tgz",
            &folder.join(format!("{STAGING_PREFIX}c")),
            folder,
            "c",
            &std::sync::atomic::AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(refused.contains(LIBRARY_FILE), "{refused}");
        assert_eq!(current_folder(folder).unwrap().join(LIBRARY_FILE), second);

        // The next download sweeps the install no longer in use.
        sweep(folder);
        assert!(!first.exists());
        assert!(second.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn an_old_install_that_cannot_be_removed_does_not_stop_a_new_one() {
        use std::os::unix::fs::PermissionsExt;
        let folder = tempfile::tempdir().unwrap();
        let folder = folder.path();
        let first = install(folder, "a", b"first").unwrap();
        let old = first.parent().unwrap().to_path_buf();
        // As a loaded library does on Windows: nothing in the folder can be
        // deleted.
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o555)).unwrap();
        let second = install(folder, "b", b"second").unwrap();
        sweep(folder);
        let kept = first.is_file();
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(current_folder(folder).unwrap().join(LIBRARY_FILE), second);
        assert_eq!(std::fs::read(&second).unwrap(), b"second");
        // Root removes it anyway; anyone else leaves it whole for later.
        if kept {
            assert_eq!(std::fs::read(&first).unwrap(), b"first");
        }
    }

    #[test]
    fn a_stopped_download_installs_nothing_and_leaves_nothing_behind() {
        let folder = tempfile::tempdir().unwrap();
        let folder = folder.path();
        let first = install(folder, "a", b"first").unwrap();
        // Stop drops the download while it waits for the archive or the
        // unpack: the guard removes both, and tells the unpack to discard.
        let cleanup = Cleanup {
            archive: folder.join(format!(".ort.tgz{PART_SUFFIX}")),
            staging: folder.join(format!("{STAGING_PREFIX}b")),
            cancelled: std::sync::Arc::default(),
        };
        std::fs::write(&cleanup.archive, tgz(&[(library_entry(), File(b"second"))])).unwrap();
        let (archive, staging) = (cleanup.archive.clone(), cleanup.staging.clone());
        let cancelled = std::sync::Arc::clone(&cleanup.cancelled);
        std::fs::create_dir_all(&staging).unwrap();
        drop(cleanup);
        assert!(!archive.exists());
        assert!(!staging.exists());
        // An unpack that was already running finishes its work and then
        // throws it away.
        std::fs::write(&archive, tgz(&[(library_entry(), File(b"second"))])).unwrap();
        let stopped =
            install_archive(&archive, "ort.tgz", &staging, folder, "b", &cancelled).unwrap_err();
        assert!(stopped.contains("stopped"), "{stopped}");
        assert!(!staging.exists());
        assert_eq!(current_folder(folder).unwrap().join(LIBRARY_FILE), first);
        // What a crashed download left is swept by the next one.
        std::fs::create_dir_all(folder.join(format!("{STAGING_PREFIX}crashed"))).unwrap();
        std::fs::write(folder.join(format!(".x.zip{PART_SUFFIX}")), b"half").unwrap();
        std::fs::remove_file(&archive).unwrap();
        sweep(folder);
        let left: Vec<String> = std::fs::read_dir(folder)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with('.'))
            .collect();
        assert!(left.is_empty(), "{left:?}");
    }

    #[tokio::test]
    async fn an_installed_release_is_not_downloaded_again_unless_asked() {
        let folder = tempfile::tempdir().unwrap();
        let config = Config {
            onnxruntime: None,
            folder: folder.path().to_path_buf(),
            use_gpu: true,
        };
        let library = install(folder.path(), "a", b"first").unwrap();
        let release = Release {
            asset: "ort.tgz",
            sha256: "",
        };
        // The address answers nothing: reaching for it would fail.
        let nowhere = "http://127.0.0.1:9/ort.tgz";
        let said = std::sync::Mutex::new(Vec::new());
        let report = |step: Step| said.lock().unwrap().push(step.message);
        let kept = download_runtime_from(&config, release, nowhere, false, &report)
            .await
            .unwrap();
        assert_eq!(
            kept,
            RuntimeInstall {
                library: library.clone(),
                already_installed: true
            }
        );
        assert!(said.lock().unwrap()[0].contains("already installed"));
        // Forced, it tries the download, which fails here and leaves the
        // install in use as it was, with no archive behind.
        assert!(
            download_runtime_from(&config, release, nowhere, true, &report)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&library).unwrap(), b"first");
        assert!(!folder
            .path()
            .join(format!(".ort.tgz{PART_SUFFIX}"))
            .exists());
    }
}
