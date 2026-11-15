//! Downloads of ONNX Runtime and the noise model. Both start only when a
//! person asks: a button in Settings on the server, a command on a worker.
//!
//! The model comes through Seiza, which verifies each file against its
//! manifest. Seiza does not fetch ONNX Runtime, so PSF Guard downloads
//! Microsoft's release archive for this platform, checks it against the
//! SHA-256 below, and unpacks the libraries into its own folder.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

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

/// Download this platform's ONNX Runtime into `config`'s folder and return
/// the library's path. The archive is verified before anything is unpacked,
/// and the folder appears whole or not at all.
pub async fn download_runtime(
    config: &Config,
    report: &(dyn Fn(Step) + Send + Sync),
) -> Result<PathBuf, String> {
    let release = release().ok_or_else(|| NO_RELEASE.to_string())?;
    download_runtime_from(
        config,
        release,
        &format!("{RELEASE_URL}/v{ONNXRUNTIME_VERSION}/{}", release.asset),
        report,
    )
    .await
}

async fn download_runtime_from(
    config: &Config,
    release: Release,
    url: &str,
    report: &(dyn Fn(Step) + Send + Sync),
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(&config.folder)
        .map_err(|error| format!("Creating {}: {error}", config.folder.display()))?;
    let tag = std::process::id();
    let archive = config.folder.join(format!(".{}.{tag}.part", release.asset));
    let staging = config.folder.join(format!(".onnxruntime-{tag}"));
    let fetched = fetch(url, &archive, release, report).await;
    let installed = match fetched {
        Ok(()) => {
            report(Step::say(format!(
                "Unpacking ONNX Runtime {ONNXRUNTIME_VERSION}…"
            )));
            let target = config.runtime_folder();
            let (archive, staging) = (archive.clone(), staging.clone());
            tokio::task::spawn_blocking(move || {
                install_archive(&archive, release.asset, &staging, &target)
            })
            .await
            .map_err(|error| format!("Unpacking ONNX Runtime failed: {error}"))
            .and_then(|installed| installed)
        }
        Err(error) => Err(error),
    };
    let _ = std::fs::remove_file(&archive);
    let _ = std::fs::remove_dir_all(&staging);
    installed
}

/// Download `url` to `archive` and check it against the release's hash.
async fn fetch(
    url: &str,
    archive: &Path,
    release: Release,
    report: &(dyn Fn(Step) + Send + Sync),
) -> Result<(), String> {
    use tokio::io::AsyncWriteExt;

    let failed = |error: &dyn std::fmt::Display| format!("Downloading {}: {error}", release.asset);
    let client = reqwest::Client::builder()
        .user_agent(concat!("psf-guard/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| failed(&error))?;
    let mut response = client
        .get(url)
        .send()
        .await
        .and_then(|response| response.error_for_status())
        .map_err(|error| failed(&error))?;
    let total = response.content_length();
    let mut file = tokio::fs::File::create(archive)
        .await
        .map_err(|error| format!("Creating {}: {error}", archive.display()))?;
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    let message = format!("Downloading ONNX Runtime {ONNXRUNTIME_VERSION}…");
    while let Some(chunk) = response.chunk().await.map_err(|error| failed(&error))? {
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|error| format!("Writing {}: {error}", archive.display()))?;
        done += chunk.len() as u64;
        report(Step {
            message: message.clone(),
            bytes_done: Some(done),
            bytes_total: total,
        });
    }
    file.flush()
        .await
        .map_err(|error| format!("Writing {}: {error}", archive.display()))?;
    drop(file);
    let found = crate::remote_work::hex(&hasher.finalize());
    if found != release.sha256 {
        return Err(format!(
            "{} does not match its SHA-256: expected {}, found {found}. Nothing was installed",
            release.asset, release.sha256
        ));
    }
    Ok(())
}

/// Unpack a verified archive into `staging`, then put that in place of
/// `target`. Returns the library's path in `target`.
fn install_archive(
    archive: &Path,
    asset: &str,
    staging: &Path,
    target: &Path,
) -> Result<PathBuf, String> {
    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging)
        .map_err(|error| format!("Creating {}: {error}", staging.display()))?;
    let unpacked = if asset.ends_with(".zip") {
        unpack_zip(archive, staging)
    } else {
        unpack_tgz(archive, staging)
    };
    unpacked.map_err(|error| format!("Unpacking {asset}: {error}"))?;
    if !staging.join(LIBRARY_FILE).is_file() {
        return Err(format!("{asset} has no {LIBRARY_FILE}"));
    }
    std::fs::write(staging.join("VERSION"), ONNXRUNTIME_VERSION)
        .map_err(|error| format!("Writing {}: {error}", staging.display()))?;
    if target.exists() {
        // Windows refuses to remove a library a process has open.
        std::fs::remove_dir_all(target).map_err(|error| {
            format!(
                "Replacing {}: {error}. Stop PSF Guard and every worker that uses it, then download again",
                target.display()
            )
        })?;
    }
    std::fs::rename(staging, target)
        .map_err(|error| format!("Moving ONNX Runtime to {}: {error}", target.display()))?;
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

    #[test]
    fn an_install_replaces_the_folder_whole_and_refuses_an_archive_without_the_library() {
        let folder = tempfile::tempdir().unwrap();
        let target = folder.path().join("onnxruntime");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("old"), b"old").unwrap();
        let library = if LIBRARY_FILE.ends_with(".dll") {
            "x/lib/onnxruntime.dll"
        } else if LIBRARY_FILE.ends_with(".dylib") {
            "x/lib/libonnxruntime.dylib"
        } else {
            "x/lib/libonnxruntime.so"
        };
        // The test archive is a tgz on every platform; only the name the
        // platform loads differs.
        let archive = folder.path().join("ort.tgz");
        std::fs::write(&archive, tgz(&[(library, File(b"library"))])).unwrap();
        let staging = folder.path().join(".staging");
        let installed = install_archive(&archive, "ort.tgz", &staging, &target).unwrap();
        assert_eq!(installed, target.join(LIBRARY_FILE));
        assert!(!target.join("old").exists());
        assert_eq!(
            std::fs::read_to_string(target.join("VERSION")).unwrap(),
            ONNXRUNTIME_VERSION
        );
        assert!(!staging.exists());

        std::fs::write(&archive, tgz(&[("x/LICENSE", File(b"MIT"))])).unwrap();
        let refused = install_archive(&archive, "ort.tgz", &staging, &target).unwrap_err();
        assert!(refused.contains(LIBRARY_FILE), "{refused}");
        // The folder installed before stays.
        assert!(target.join(LIBRARY_FILE).is_file());
    }
}
