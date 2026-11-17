//! Installing an offline sky map: one of N.I.N.A.'s framing assistant
//! caches, downloaded from N.I.N.A.'s site, checked, and put in place whole.
//!
//! Settings and the command line both install through here. A download
//! starts only when a person asks for it. The archive is checked against
//! the size and SHA-256 recorded in [`SETS`] before anything is unpacked,
//! and it is unpacked beside the maps in a folder [`crate::sky_maps::discover`]
//! skips, so a running server lists a set only once it is complete.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use crate::sky_maps::{self, SkyMap};

const RELEASES: &str = "https://nighttime-imaging.eu/downloads/Setup/Releases/";
/// No archive PSF Guard is given by URL may be larger than this.
const MAX_ZIP_BYTES: u64 = 8 * 1024 * 1024 * 1024;
/// Room kept free beyond the archive and its unpacked files.
const SPARE_BYTES: u64 = 512 * 1024 * 1024;

/// One set N.I.N.A. publishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Set {
    /// The name the command line and the API use.
    pub key: &'static str,
    /// What Settings calls it.
    pub name: &'static str,
    pub file: &'static str,
    /// The folder the archive unpacks to, which is the map's folder.
    pub folder: &'static str,
    /// The archive's size and SHA-256, from the file N.I.N.A. published.
    pub bytes: u64,
    pub sha256: &'static str,
    /// What the set covers, in a few words.
    pub covers: &'static str,
    /// The terms it is offered under, when they limit its use.
    pub licence: Option<&'static str>,
}

impl Set {
    pub fn url(&self) -> String {
        format!("{RELEASES}{}", self.file)
    }
}

/// The sets on N.I.N.A.'s download page. Sizes and hashes were read from
/// the files as published on 2026-11-16 (the DSS set dated 2024-03-25, the
/// narrowband sets 2025-10-17).
pub const SETS: [Set; 3] = [
    Set {
        key: "full",
        name: "Whole sky, DSS",
        file: "FramingAssistantCache_Full.zip",
        folder: "FramingAssistantCache",
        bytes: 3_526_916_102,
        sha256: "3bdb149acd82c58399685d5bbef5163e2cb8ec9cbe1e178c7a8906642262172e",
        covers: "The whole sky in colour, from the Digitized Sky Survey",
        licence: None,
    },
    Set {
        key: "nsns-ohs",
        name: "Northern sky narrowband, with stars",
        file: "FramingAssistantCache_NorthernSkyNarrowbandSurvey_OHS_withStars.zip",
        folder: "FramingAssistantCache_NorthernSkyNarrowbandSurvey_OHS_withStars",
        bytes: 1_428_767_069,
        sha256: "230bd038a2cb8f481c34b765d668d51e2a95ea7aa250985cda094287ca366dd3",
        covers: "The northern sky in O III, Hα and S II",
        licence: Some("CC BY-NC-SA 4.0, Northern Sky Narrowband Survey"),
    },
    Set {
        key: "nsns-ohs-starless",
        name: "Northern sky narrowband, starless",
        file: "FramingAssistantCache_NorthernSkyNarrowbandSurvey_OHS_starless.zip",
        folder: "FramingAssistantCache_NorthernSkyNarrowbandSurvey_OHS_starless",
        bytes: 471_106_178,
        sha256: "1bcf879b0ce4de2fe2cb9d3e2008219ece95aac906c797423155dc769e7c7bce",
        covers: "The northern sky in O III, Hα and S II, with the stars removed",
        licence: Some("CC BY-NC-SA 4.0, Northern Sky Narrowband Survey"),
    },
];

/// The set a short name means. `dss`, `nsns` and `nsns-starless` are the
/// command line's older names.
pub fn set(key: &str) -> Option<&'static Set> {
    let key = match key {
        "dss" => "full",
        "nsns" => "nsns-ohs",
        "nsns-starless" => "nsns-ohs-starless",
        other => other,
    };
    SETS.iter().find(|set| set.key == key)
}

/// What to download: a published set, checked against its record, or an
/// archive at a URL a person gave on the command line, which has no record
/// to check against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Set(&'static Set),
    Url(String),
}

impl Source {
    fn url(&self) -> String {
        match self {
            Self::Set(set) => set.url(),
            Self::Url(url) => url.clone(),
        }
    }

    /// The size and SHA-256 on record for it, when it is a published set.
    fn record(&self) -> Option<(u64, &'static str)> {
        match self {
            Self::Set(set) => Some((set.bytes, set.sha256)),
            Self::Url(_) => None,
        }
    }
}

/// How far an install has got.
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

/// A name no other install uses: the process and the time.
fn tag() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos:x}", std::process::id())
}

/// The file an install or a move locks for as long as it changes a maps
/// folder. Settings and the command line may both install into the same
/// folder; the lock keeps one from removing the other's files, and the
/// system releases it if the process ends.
const LOCK_FILE: &str = ".install.lock";

/// The lock on `root`, held while the returned file is open.
fn lock(root: &Path) -> Result<std::fs::File, String> {
    std::fs::create_dir_all(root)
        .map_err(|error| format!("Creating {}: {error}", root.display()))?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(root.join(LOCK_FILE))
        .map_err(|error| format!("Opening {}: {error}", root.join(LOCK_FILE).display()))?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(format!(
            "Another install or move is changing {}. Wait for it to end, then try again.",
            root.display()
        )),
        Err(std::fs::TryLockError::Error(error)) => {
            Err(format!("Locking {}: {error}", root.display()))
        }
    }
}

/// Leftovers of installs and moves that did not finish: archives and
/// unpack folders. Only called with [`lock`] held, so nothing it removes
/// belongs to work still under way. [`sky_maps::discover`] passes over the
/// folders meanwhile.
fn sweep(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".download-") {
            let _ = std::fs::remove_file(entry.path());
        } else if name.starts_with(sky_maps::UNPACKING) || name.starts_with(sky_maps::REPLACED) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The room an install needs in `root`'s volume: the archive, its unpacked
/// files (JPEGs, which a zip does not shrink) and some to spare.
pub fn room_needed(archive_bytes: u64) -> u64 {
    archive_bytes.saturating_mul(2).saturating_add(SPARE_BYTES)
}

fn gigabytes(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
}

/// Refuse an install that `root`'s volume has no room for. A volume whose
/// free space cannot be read is let through.
fn check_room(root: &Path, archive_bytes: u64) -> Result<(), String> {
    let needed = room_needed(archive_bytes);
    match crate::server::cache_budget::available_bytes(root) {
        Some(free) if free < needed => Err(format!(
            "{} has {} free and this map needs {} while it installs. Free some space, or \
             choose another folder for offline sky maps.",
            root.display(),
            gigabytes(free),
            gigabytes(needed)
        )),
        _ => Ok(()),
    }
}

/// Download `source` into `root` and return the installed map. `report` is
/// told how far it has got. `stop` ends the download at once, and the
/// unpack between files and once more just before the map is put in place;
/// an install that has passed that point finishes. `while_unpacking` is
/// called as the unpack starts and its value kept until the unpack ends,
/// as the server holds the interactive guard for that part only. A set
/// installed already is replaced only when the new one is whole. However it
/// ends, the install's archive and unpack folder are removed before this
/// returns.
pub async fn install<G: Send + 'static>(
    root: &Path,
    source: Source,
    stop: crate::concurrency::Stop,
    while_unpacking: impl FnOnce() -> G + Send + 'static,
    report: &(dyn Fn(Step) + Send + Sync),
) -> Result<SkyMap, String> {
    let lock = lock(root)?;
    sweep(root);
    let record = source.record();
    if let Some((bytes, _)) = record {
        check_room(root, bytes)?;
    }
    let tag = tag();
    let archive = root.join(format!(".download-{tag}.zip"));
    let staging = root.join(format!("{}{tag}", sky_maps::UNPACKING));
    let url = source.url();
    // The download only writes the archive, so it may be dropped where it
    // stands, as Stop does while it waits on the network. A finished
    // download wins over a stop that arrives with it: the unpack sees that
    // stop.
    let stopped = || stop.requested();
    let fetched = tokio::select! {
        biased;
        fetched = fetch(&url, record, &archive, root, &stopped, report) => fetched,
        () = stop.stopped() => Err("Stopped".to_string()),
    };
    let target_root = root.to_path_buf();
    // The unpack, the switch and the clean-up all run on one blocking
    // thread, which this waits for: nothing removes the unpack folder while
    // the unpack still writes to it.
    tokio::task::spawn_blocking(move || {
        let placed = fetched.and_then(|()| {
            let _held = while_unpacking();
            place(&archive, &staging, &target_root, &|| stop.requested())
        });
        remove_soon(&archive);
        let _ = std::fs::remove_dir_all(&staging);
        drop(lock);
        placed
    })
    .await
    .map_err(|error| format!("Unpacking failed: {error}"))?
}

/// Stream `url` to `archive`. With a `record`, the size and SHA-256 a
/// published set must have, the file is checked against it.
async fn fetch(
    url: &str,
    record: Option<(u64, &str)>,
    archive: &Path,
    root: &Path,
    stopped: &(dyn Fn() -> bool + Send + Sync),
    report: &(dyn Fn(Step) + Send + Sync),
) -> Result<(), String> {
    use crate::verified_download::{Expected, Failure};

    let reported =
        std::sync::Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(1));
    let progress = |done: u64, total: Option<u64>| {
        let mut last = reported
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last.elapsed().as_millis() < 500 && Some(done) != total {
            return;
        }
        *last = std::time::Instant::now();
        report(Step {
            message: "Downloading…".into(),
            bytes_done: Some(done),
            bytes_total: total,
        });
    };
    crate::verified_download::fetch(
        url,
        archive,
        Expected {
            bytes: record.map(|(bytes, _)| bytes),
            sha256: record.map(|(_, sha256)| sha256),
        },
        MAX_ZIP_BYTES,
        stopped,
        // A published set's room was checked before; a zip given by URL is
        // checked once the server gives its size.
        &|bytes| match record {
            Some(_) => Ok(()),
            None => check_room(root, bytes),
        },
        &progress,
    )
    .await
    .map_err(|failure| match failure {
        Failure::Transfer(error) => format!("{url} could not be downloaded: {error}"),
        Failure::Write(error) => format!("Writing {error}"),
        Failure::TooLarge { limit } => format!("{url} is larger than {}", gigabytes(limit)),
        Failure::NotOnRecord { .. } => changed(url),
        Failure::Refused(error) => error,
        Failure::Stopped => "Stopped".into(),
    })?;
    report(Step::say("Unpacking…"));
    Ok(())
}

/// Remove a file another handle may still hold for a moment: a download
/// dropped mid-write on Windows keeps its file open until the write ends.
fn remove_soon(path: &Path) {
    for _ in 0..50 {
        match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            _ => return,
        }
    }
}

/// The message when a published set is not the file PSF Guard has a record
/// of: N.I.N.A. replaced it, or the download was damaged.
fn changed(url: &str) -> String {
    format!(
        "{url} is not the file this version of PSF Guard expects, so it was not installed. \
         Try again; if it fails again, N.I.N.A. has replaced the file and a PSF Guard update is \
         needed."
    )
}

/// Unpack `archive` into `staging`, check that it holds a map, and move the
/// map's folder into `root`, in place of one of the same name.
fn place(
    archive: &Path,
    staging: &Path,
    root: &Path,
    stopped: &dyn Fn() -> bool,
) -> Result<SkyMap, String> {
    let unpacked = unpack(archive, staging, stopped)?;
    SkyMap::open(&unpacked)
        .map_err(|error| format!("The archive is not a framing assistant cache: {error}"))?;
    let folder = unpacked
        .file_name()
        .ok_or_else(|| "The archive's map folder has no name".to_string())?
        .to_owned();
    if folder.to_string_lossy().starts_with('.') {
        return Err("The archive's map folder begins with a dot".into());
    }
    let target = root.join(&folder);
    // The last moment a stop counts: past it, the install finishes.
    if stopped() {
        return Err("Stopped".into());
    }
    // A set of this name is moved aside first, so the folder is never half
    // old and half new. A server may still read the old files; they go when
    // nothing holds them, now or at the next install's sweep.
    let replaced = root.join(format!("{}{}", sky_maps::REPLACED, tag()));
    let had_old = target.exists();
    if had_old {
        std::fs::rename(&target, &replaced)
            .map_err(|error| format!("Replacing {}: {error}", target.display()))?;
    }
    if let Err(error) = std::fs::rename(&unpacked, &target) {
        if had_old {
            let _ = std::fs::rename(&replaced, &target);
        }
        return Err(format!("Installing {}: {error}", target.display()));
    }
    let _ = std::fs::remove_dir_all(&replaced);
    SkyMap::open(&target)
}

/// Unpack into `staging`, refusing entries that would leave it, and return
/// the folder that holds `CacheInfo.xml`.
fn unpack(archive: &Path, staging: &Path, stopped: &dyn Fn() -> bool) -> Result<PathBuf, String> {
    let file = std::fs::File::open(archive).map_err(|error| error.to_string())?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|error| format!("Reading the archive: {error}"))?;
    let mut index_dir: Option<PathBuf> = None;
    let mut buffer = vec![0u8; 1 << 16];
    for i in 0..zip.len() {
        if stopped() {
            return Err("Stopped".into());
        }
        let mut entry = zip.by_index(i).map_err(|error| error.to_string())?;
        let Some(relative) = entry.enclosed_name() else {
            return Err("An entry of the archive would leave the map's folder".into());
        };
        let target = staging.join(&relative);
        if entry.is_dir() {
            std::fs::create_dir_all(&target).map_err(|error| error.to_string())?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut out = std::fs::File::create(&target)
            .map_err(|error| format!("Writing {}: {error}", target.display()))?;
        loop {
            let read = entry.read(&mut buffer).map_err(|error| error.to_string())?;
            if read == 0 {
                break;
            }
            out.write_all(&buffer[..read])
                .map_err(|error| format!("Writing {}: {error}", target.display()))?;
        }
        if relative.file_name().and_then(|name| name.to_str()) == Some("CacheInfo.xml") {
            index_dir = target.parent().map(Path::to_path_buf);
        }
    }
    let index_dir = index_dir.ok_or_else(|| "The archive holds no CacheInfo.xml".to_string())?;
    // The index sits in the map's folder; an archive with the index at its
    // top has no folder of its own to move.
    if index_dir == staging {
        return Err("The archive has no folder around its CacheInfo.xml".into());
    }
    Ok(index_dir)
}

/// Remove the installed map in `folder` of `root`. The folder's name is
/// checked to be a map's own: one path part, no dot in front.
pub fn remove(root: &Path, folder: &str) -> Result<(), String> {
    let dir = map_dir(root, folder)?;
    SkyMap::open(&dir).map_err(|_| format!("{folder} is not an offline sky map"))?;
    let _lock = lock(root)?;
    std::fs::remove_dir_all(&dir).map_err(|error| format!("Removing {}: {error}", dir.display()))
}

/// Move the map in `folder` of `from` into `to`, as an older install under
/// the cache folder is moved to the maps' own folder. A move across volumes
/// copies, then removes the old files.
pub fn relocate(from: &Path, folder: &str, to: &Path) -> Result<SkyMap, String> {
    let source = map_dir(from, folder)?;
    SkyMap::open(&source).map_err(|_| format!("{folder} is not an offline sky map"))?;
    // Held for the move, so no install sweeps its copy away.
    let _lock = lock(to)?;
    let target = to.join(folder);
    if target.exists() {
        return Err(format!(
            "{} already holds a map named {folder}",
            to.display()
        ));
    }
    if std::fs::rename(&source, &target).is_err() {
        let size = folder_bytes(&source);
        check_room(to, size / 2)?;
        sweep(to);
        let staging = to.join(format!("{}{}", sky_maps::UNPACKING, tag()));
        let copied = copy_tree(&source, &staging.join(folder)).and_then(|()| {
            std::fs::rename(staging.join(folder), &target).map_err(|error| error.to_string())
        });
        let _ = std::fs::remove_dir_all(&staging);
        copied.map_err(|error| format!("Moving {folder}: {error}"))?;
        let _ = std::fs::remove_dir_all(&source);
    }
    SkyMap::open(&target)
}

fn map_dir(root: &Path, folder: &str) -> Result<PathBuf, String> {
    let name = Path::new(folder);
    let single = name.components().count() == 1
        && matches!(
            name.components().next(),
            Some(std::path::Component::Normal(_))
        );
    if !single || folder.starts_with('.') {
        return Err(format!("{folder} is not a map's folder name"));
    }
    Ok(root.join(folder))
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(from).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// The bytes the files under `dir` take.
pub fn folder_bytes(dir: &Path) -> u64 {
    crate::server::cache_budget::tree_bytes(dir)
}

/// The published set an installed map's folder belongs to.
pub fn set_of(map: &SkyMap) -> Option<&'static Set> {
    let folder = map.dir.file_name()?.to_str()?;
    SETS.iter().find(|set| set.folder == folder)
}

/// Every installed map, in the maps' own folder first and then in the older
/// place under the cache, with the root each is in.
pub fn installed(cache_root: &Path) -> Vec<(SkyMap, PathBuf)> {
    let own = sky_maps::data_root();
    let older = sky_maps::root_for(cache_root);
    let mut maps: Vec<(SkyMap, PathBuf)> = sky_maps::discover(&own)
        .into_iter()
        .map(|map| (map, own.clone()))
        .collect();
    if older != own {
        for map in sky_maps::discover(&older) {
            if !maps.iter().any(|(kept, _)| kept.id == map.id) {
                maps.push((map, older.clone()));
            }
        }
    }
    maps
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    /// An install nothing stops.
    fn go() -> crate::concurrency::Stop {
        crate::concurrency::Stop::new()
    }

    /// A zip holding a one-tile map in `folder`.
    pub(crate) fn map_zip(folder: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let options = zip::write::SimpleFileOptions::default();
            zip.start_file(format!("{folder}/CacheInfo.xml"), options)
                .unwrap();
            zip.write_all(
                br#"<ImageCacheInfo><Image RA="0" Dec="0" FoVW="300" FoVH="300" Rotation="0" Source="NASA" FileName="tile.jpg" Name="tile"/></ImageCacheInfo>"#,
            )
            .unwrap();
            zip.start_file(format!("{folder}/tile.jpg"), options)
                .unwrap();
            zip.write_all(b"not really a jpeg").unwrap();
            zip.finish().unwrap();
        }
        bytes
    }

    fn sha256(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    /// Serve `body` once per request at a local address.
    async fn serve(body: Vec<u8>) -> String {
        use axum::routing::get;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/map.zip",
            get(move || {
                let body = body.clone();
                async move { body }
            }),
        );
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{address}/map.zip")
    }

    fn quiet() -> impl Fn(Step) + Send + Sync {
        |_| {}
    }

    #[test]
    fn every_published_set_has_its_size_and_hash_on_record() {
        for set in SETS {
            assert!(set.bytes > 100 * 1024 * 1024, "{}", set.key);
            assert_eq!(set.sha256.len(), 64, "{}", set.key);
            assert!(set.sha256.bytes().all(|byte| byte.is_ascii_hexdigit()));
            assert!(set.url().starts_with("https://nighttime-imaging.eu/"));
        }
        assert_eq!(set("dss").map(|set| set.key), Some("full"));
        assert_eq!(set("nsns").map(|set| set.key), Some("nsns-ohs"));
        assert_eq!(
            set("nsns-starless").map(|set| set.key),
            Some("nsns-ohs-starless")
        );
        assert_eq!(set("elsewhere"), None);
    }

    #[tokio::test]
    async fn an_archive_is_unpacked_aside_and_appears_whole() {
        let root = tempfile::tempdir().unwrap();
        let url = serve(map_zip("FramingAssistantCache")).await;
        let map = install(root.path(), Source::Url(url), go(), || (), &quiet())
            .await
            .unwrap();
        assert_eq!(map.dir, root.path().join("FramingAssistantCache"));
        assert_eq!(sky_maps::discover(root.path()).len(), 1);
        // Nothing of the install is left beside the map but the lock file.
        let mut names: Vec<String> = std::fs::read_dir(root.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, [LOCK_FILE, "FramingAssistantCache"]);
    }

    #[tokio::test]
    async fn a_file_that_is_not_the_one_on_record_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let body = map_zip("FramingAssistantCache");
        let (bytes, hash) = (body.len() as u64, sha256(&body));
        let url = serve(body).await;
        let archive = root.path().join(".download-test.zip");
        let check = |record: (u64, String)| {
            let (url, archive, root) = (url.clone(), archive.clone(), root.path().to_path_buf());
            async move {
                fetch(
                    &url,
                    Some((record.0, &record.1)),
                    &archive,
                    &root,
                    &|| false,
                    &quiet(),
                )
                .await
            }
        };
        // The file on record passes.
        check((bytes, hash.clone())).await.unwrap();
        // Another hash, a larger size and a smaller size do not.
        for record in [
            (bytes, "0".repeat(64)),
            (bytes + 1, hash.clone()),
            (bytes - 1, hash.clone()),
        ] {
            let refused = check(record).await.unwrap_err();
            assert!(refused.contains("not the file this version"), "{refused}");
        }
    }

    #[tokio::test]
    async fn a_new_install_replaces_a_set_of_the_same_name_only_when_it_is_whole() {
        let root = tempfile::tempdir().unwrap();
        let url = serve(map_zip("FramingAssistantCache")).await;
        install(root.path(), Source::Url(url.clone()), go(), || (), &quiet())
            .await
            .unwrap();
        std::fs::write(root.path().join("FramingAssistantCache/mine.txt"), b"old").unwrap();
        // A broken archive leaves the installed set as it was.
        let broken = serve(b"not a zip".to_vec()).await;
        assert!(
            install(root.path(), Source::Url(broken), go(), || (), &quiet())
                .await
                .is_err()
        );
        assert!(root.path().join("FramingAssistantCache/mine.txt").exists());
        // A whole one takes its place.
        install(root.path(), Source::Url(url), go(), || (), &quiet())
            .await
            .unwrap();
        assert!(!root.path().join("FramingAssistantCache/mine.txt").exists());
        assert_eq!(sky_maps::discover(root.path()).len(), 1);
    }

    #[tokio::test]
    async fn a_map_still_being_unpacked_or_moved_aside_is_not_listed() {
        let root = tempfile::tempdir().unwrap();
        let url = serve(map_zip("FramingAssistantCache")).await;
        install(root.path(), Source::Url(url), go(), || (), &quiet())
            .await
            .unwrap();
        for aside in [".unpack-1-a", ".replaced-1-b"] {
            copy_tree(
                &root.path().join("FramingAssistantCache"),
                &root.path().join(aside),
            )
            .unwrap();
        }
        let listed = sky_maps::discover(root.path());
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].dir, root.path().join("FramingAssistantCache"));
    }

    #[tokio::test]
    async fn a_second_install_into_the_same_folder_waits_its_turn() {
        let root = tempfile::tempdir().unwrap();
        let held = lock(root.path()).unwrap();
        let url = serve(map_zip("FramingAssistantCache")).await;
        let refused = install(root.path(), Source::Url(url.clone()), go(), || (), &quiet())
            .await
            .unwrap_err();
        assert!(refused.contains("Another install or move"), "{refused}");
        // Its files were not touched by the refused install's sweep.
        std::fs::write(root.path().join(".download-other.zip"), b"theirs").unwrap();
        assert!(
            install(root.path(), Source::Url(url.clone()), go(), || (), &quiet())
                .await
                .is_err()
        );
        assert!(root.path().join(".download-other.zip").exists());
        drop(held);
        install(root.path(), Source::Url(url), go(), || (), &quiet())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_stop_during_the_unpack_leaves_the_installed_set_and_nothing_else() {
        let root = tempfile::tempdir().unwrap();
        let url = serve(map_zip("FramingAssistantCache")).await;
        install(root.path(), Source::Url(url.clone()), go(), || (), &quiet())
            .await
            .unwrap();
        std::fs::write(root.path().join("FramingAssistantCache/mine.txt"), b"old").unwrap();
        // Stopped as the download ends, so only the unpack can see it.
        let stop = crate::concurrency::Stop::new();
        let asked = stop.clone();
        let stop_at_unpack = move |step: Step| {
            if step.message == "Unpacking…" {
                asked.request();
            }
        };
        let unpacking = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = unpacking.clone();
        let ended = install(
            root.path(),
            Source::Url(url),
            stop,
            move || seen.store(true, std::sync::atomic::Ordering::Relaxed),
            &stop_at_unpack,
        )
        .await;
        assert_eq!(ended.unwrap_err(), "Stopped");
        assert!(
            unpacking.load(std::sync::atomic::Ordering::Relaxed),
            "the stop was seen by the unpack, not the download"
        );
        assert!(root.path().join("FramingAssistantCache/mine.txt").exists());
        let names: Vec<String> = std::fs::read_dir(root.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != LOCK_FILE)
            .collect();
        assert_eq!(names, ["FramingAssistantCache"]);
    }

    #[test]
    fn leftovers_are_swept_and_a_map_is_left_alone() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join(".download-1-a.zip"), b"part").unwrap();
        std::fs::create_dir_all(root.path().join(".unpack-1-a/inner")).unwrap();
        std::fs::create_dir_all(root.path().join(".replaced-1-b")).unwrap();
        std::fs::create_dir_all(root.path().join("FramingAssistantCache")).unwrap();
        sweep(root.path());
        let names: Vec<String> = std::fs::read_dir(root.path())
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["FramingAssistantCache"]);
    }

    #[test]
    fn an_archive_that_would_leave_its_folder_or_has_no_map_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let never = || false;
        // No index at all.
        let mut bytes = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            zip.start_file("readme.txt", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"hello").unwrap();
            zip.finish().unwrap();
        }
        let archive = root.path().join("a.zip");
        std::fs::write(&archive, &bytes).unwrap();
        assert!(unpack(&archive, &root.path().join(".unpack-a"), &never)
            .unwrap_err()
            .contains("no CacheInfo.xml"));
        // A stopped unpack writes nothing more.
        std::fs::write(&archive, map_zip("FramingAssistantCache")).unwrap();
        let stopped = || true;
        assert_eq!(
            unpack(&archive, &root.path().join(".unpack-b"), &stopped).unwrap_err(),
            "Stopped"
        );
    }

    #[test]
    fn only_a_maps_own_folder_can_be_removed_or_moved() {
        let root = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        for bad in ["..", "a/b", ".unpack-1", ""] {
            assert!(remove(root.path(), bad).is_err(), "{bad}");
            assert!(relocate(root.path(), bad, other.path()).is_err(), "{bad}");
        }
        // A folder that is not a map is not removed.
        std::fs::create_dir_all(root.path().join("photos")).unwrap();
        assert!(remove(root.path(), "photos").is_err());
        assert!(root.path().join("photos").exists());
    }

    #[tokio::test]
    async fn a_map_under_the_cache_moves_to_the_maps_own_folder() {
        let cache = tempfile::tempdir().unwrap();
        let own = tempfile::tempdir().unwrap();
        let url = serve(map_zip("FramingAssistantCache")).await;
        install(cache.path(), Source::Url(url), go(), || (), &quiet())
            .await
            .unwrap();
        let moved = relocate(cache.path(), "FramingAssistantCache", own.path()).unwrap();
        assert_eq!(moved.dir, own.path().join("FramingAssistantCache"));
        assert!(sky_maps::discover(cache.path()).is_empty());
        // A second map of that name is not written over.
        let url = serve(map_zip("FramingAssistantCache")).await;
        install(cache.path(), Source::Url(url), go(), || (), &quiet())
            .await
            .unwrap();
        assert!(relocate(cache.path(), "FramingAssistantCache", own.path()).is_err());
    }

    #[test]
    fn an_install_needs_room_for_the_archive_and_its_files() {
        assert_eq!(room_needed(1_000), 2_000 + SPARE_BYTES);
    }
}
