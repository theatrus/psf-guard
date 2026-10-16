use super::*;
use crate::db_registry::{DbRegistry, PeerEntry};
use chrono::TimeZone;
use rusqlite::Connection;
use std::io::Read;

struct Setup {
    _dir: tempfile::TempDir,
    root: PathBuf,
    locations: Locations,
    registry: DbRegistry,
}

fn catalog(path: &Path, frames: usize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let connection = Connection::open(path).unwrap();
    connection
        .execute_batch(
            "PRAGMA user_version = 21;
             CREATE TABLE project (Id INTEGER PRIMARY KEY, name TEXT);
             CREATE TABLE acquiredimage (Id INTEGER PRIMARY KEY, projectId INTEGER);
             CREATE TABLE psf_guard_calibration_schema (singleton INTEGER PRIMARY KEY, version INTEGER);
             INSERT INTO psf_guard_calibration_schema VALUES (1, 9);",
        )
        .unwrap();
    for _ in 0..frames {
        connection
            .execute("INSERT INTO acquiredimage (projectId) VALUES (1)", [])
            .unwrap();
    }
}

/// A registry with two catalogs (one gone), a peer key, accounts, a
/// collaboration credentials file, a Director store and a server config.
fn setup() -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let config = root.join("config");
    std::fs::create_dir_all(&config).unwrap();
    let registry_path = config.join("config.json");
    let mut registry = DbRegistry::default();
    let rig = root.join("rig").join("schedulerdb.sqlite");
    catalog(&rig, 12);
    registry
        .add(
            "Imaging Rig".into(),
            rig.display().to_string(),
            vec![root.join("images").display().to_string()],
            Some("imaging-rig".into()),
        )
        .unwrap();
    registry
        .add(
            "Gone".into(),
            root.join("gone")
                .join("missing.sqlite")
                .display()
                .to_string(),
            vec![],
            Some("gone".into()),
        )
        .unwrap();
    registry.peers.push(PeerEntry {
        id: "scope".into(),
        name: "Scope".into(),
        base_url: "https://scope.example".into(),
        token: "psfg-peer-secret-0123456789".into(),
        catalog_id: None,
    });
    registry.save(&registry_path).unwrap();
    std::fs::write(
        config.join("auth.json"),
        r#"{"schema_version":1,"users":[]}"#,
    )
    .unwrap();
    std::fs::write(config.join("user-preferences.json"), "{}").unwrap();
    std::fs::write(
        config.join("collaboration-credentials.json"),
        r#"{"version":1,"entries":[]}"#,
    )
    .unwrap();
    let director = Connection::open(config.join("director-meta.sqlite")).unwrap();
    director
        .execute_batch("CREATE TABLE plan (id INTEGER PRIMARY KEY); INSERT INTO plan VALUES (1);")
        .unwrap();
    drop(director);
    let toml = root.join("psf-guard.toml");
    std::fs::write(
        &toml,
        "[server]\nport = 3000\n\n[[remote_sync]]\ndatabase = \"imaging-rig\"\ntoken = \"a-remote-sync-token-long-enough\"\n",
    )
    .unwrap();
    let cache = root.join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(
        cache.join("remote-sync-audit.jsonl"),
        "{\"action\":\"apply\"}\n",
    )
    .unwrap();
    Setup {
        locations: Locations {
            registry: registry_path,
            server_config: Some(toml),
            director_meta: None,
            cache_root: Some(cache),
        },
        registry,
        root,
        _dir: dir,
    }
}

fn entries(path: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
    (0..archive.len())
        .map(|index| {
            let mut entry = archive.by_index(index).unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            (entry.name().unwrap().to_string(), bytes)
        })
        .collect()
}

fn sha(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn moment(seconds: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_790_000_000 + seconds, 0).unwrap()
}

#[test]
fn a_full_backup_holds_the_settings_and_every_reachable_catalog() {
    let setup = setup();
    let plan = plan(&setup.locations, &setup.registry, BackupScope::Full, false);
    let destination = setup.root.join("backups");
    let written = write(
        &plan,
        &destination,
        Trigger::Manual,
        moment(0),
        &Hooks::quiet(),
    )
    .unwrap();
    assert!(written.file_name.starts_with("psf-guard-backup-"));
    assert!(written.file_name.ends_with("-full.zip"));
    assert!(is_archive_name(&written.file_name));

    let files = entries(&written.path);
    let names: Vec<&str> = files.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        vec![
            "README.txt",
            "catalogs/imaging-rig/schedulerdb.sqlite",
            "manifest.json",
            "meta/auth.json",
            "meta/config.json",
            "meta/director-meta.sqlite",
            "meta/psf-guard.toml",
            "meta/remote-sync-audit.jsonl",
            "meta/user-preferences.json",
        ]
    );

    let manifest: Manifest = serde_json::from_slice(&files["manifest.json"]).unwrap();
    assert_eq!(manifest, written.manifest);
    assert_eq!(manifest.format, FORMAT);
    assert_eq!(manifest.psf_guard_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(manifest.scope, BackupScope::Full);
    assert_eq!(manifest.trigger, Trigger::Manual);
    assert_eq!(manifest.secrets, SecretsMode::Redacted);
    // Every file's checksum and size match what the archive holds, and
    // its source says where it goes back.
    for file in &manifest.files {
        let bytes = &files[&file.path];
        assert_eq!(file.sha256, sha(bytes), "{}", file.path);
        assert_eq!(file.bytes, bytes.len() as u64, "{}", file.path);
        assert!(Path::new(&file.source).is_absolute());
    }
    let catalog = manifest
        .files
        .iter()
        .find(|file| file.role == FileRole::Catalog)
        .unwrap();
    assert_eq!(catalog.catalog_id.as_deref(), Some("imaging-rig"));
    let info = catalog.sqlite.as_ref().unwrap();
    assert_eq!(info.user_version, 21);
    assert_eq!(info.psf_guard_schema, Some(9));
    assert_eq!(info.acquired_images, Some(12));
    assert_eq!(info.quick_check, "ok");
    // The unreachable catalog is recorded, not silently dropped.
    let gone = manifest.catalogs.iter().find(|c| c.id == "gone").unwrap();
    assert!(!gone.included);
    assert!(gone.problem.as_deref().unwrap().contains("not reachable"));
    assert!(manifest
        .catalogs
        .iter()
        .any(|c| c.id == "imaging-rig" && c.included));
    // The archive's catalog opens as a database.
    let restored = setup.root.join("restored.sqlite");
    std::fs::write(&restored, &files["catalogs/imaging-rig/schedulerdb.sqlite"]).unwrap();
    let count: i64 = Connection::open(&restored)
        .unwrap()
        .query_row("SELECT COUNT(*) FROM acquiredimage", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 12);
    // Nothing of the work in progress is left behind.
    let leftovers: Vec<String> = std::fs::read_dir(&destination)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(leftovers, vec![written.file_name.clone()]);
}

#[test]
fn secrets_are_left_out_unless_asked_for() {
    let setup = setup();
    let destination = setup.root.join("backups");
    let redacted = plan(&setup.locations, &setup.registry, BackupScope::Meta, false);
    assert_eq!(redacted.omitted.len(), 1);
    assert_eq!(redacted.omitted[0].role, FileRole::CollaborationCredentials);
    let written = write(
        &redacted,
        &destination,
        Trigger::Cli,
        moment(0),
        &Hooks::quiet(),
    )
    .unwrap();
    let files = entries(&written.path);
    let registry = String::from_utf8(files["meta/config.json"].clone()).unwrap();
    assert!(!registry.contains("psfg-peer-secret"));
    let toml = String::from_utf8(files["meta/psf-guard.toml"].clone()).unwrap();
    assert!(!toml.contains("a-remote-sync-token-long-enough"));
    assert!(!files.contains_key("meta/collaboration-credentials.json"));
    let config = written
        .manifest
        .files
        .iter()
        .find(|file| file.role == FileRole::Registry)
        .unwrap();
    assert_eq!(config.redacted, vec!["1 peer key"]);
    // Meta only: catalogs are listed but not copied.
    assert!(!files.keys().any(|name| name.starts_with("catalogs/")));
    assert!(written.manifest.catalogs.iter().all(|c| !c.included));

    let kept = plan(&setup.locations, &setup.registry, BackupScope::Meta, true);
    assert!(kept.omitted.is_empty());
    let written = write(
        &kept,
        &destination,
        Trigger::Cli,
        moment(1),
        &Hooks::quiet(),
    )
    .unwrap();
    let files = entries(&written.path);
    assert!(String::from_utf8(files["meta/config.json"].clone())
        .unwrap()
        .contains("psfg-peer-secret"));
    assert!(files.contains_key("meta/collaboration-credentials.json"));
    assert_eq!(written.manifest.secrets, SecretsMode::Included);
}

#[test]
fn a_stopped_backup_leaves_nothing_in_the_folder() {
    let setup = setup();
    let plan = plan(&setup.locations, &setup.registry, BackupScope::Full, false);
    let destination = setup.root.join("backups");
    let calls = std::cell::Cell::new(0);
    let cancelled = || {
        calls.set(calls.get() + 1);
        calls.get() > 4
    };
    let hooks = Hooks {
        cancelled: &cancelled,
        ..Hooks::quiet()
    };
    let error = write(&plan, &destination, Trigger::Manual, moment(0), &hooks).unwrap_err();
    assert!(error.downcast_ref::<Cancelled>().is_some(), "{error:#}");
    assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 0);
}

#[test]
fn a_backup_never_replaces_a_file_already_there() {
    let setup = setup();
    let plan = plan(&setup.locations, &setup.registry, BackupScope::Meta, false);
    let destination = setup.root.join("backups");
    std::fs::create_dir_all(&destination).unwrap();
    let name = archive_name(moment(0), BackupScope::Meta, Trigger::Manual);
    std::fs::write(destination.join(&name), "someone else's").unwrap();
    let written = write(
        &plan,
        &destination,
        Trigger::Manual,
        moment(0),
        &Hooks::quiet(),
    )
    .unwrap();
    assert_ne!(written.file_name, name);
    assert!(is_archive_name(&written.file_name));
    assert_eq!(
        std::fs::read_to_string(destination.join(&name)).unwrap(),
        "someone else's"
    );
}

#[test]
fn progress_reaches_the_whole_and_files_are_counted() {
    let setup = setup();
    let plan = plan(&setup.locations, &setup.registry, BackupScope::Full, false);
    let seen = std::cell::RefCell::new(Vec::new());
    let progress = |p: &Progress| seen.borrow_mut().push(p.clone());
    let hooks = Hooks {
        progress: &progress,
        ..Hooks::quiet()
    };
    write(
        &plan,
        &setup.root.join("b"),
        Trigger::Manual,
        moment(0),
        &hooks,
    )
    .unwrap();
    let seen = seen.into_inner();
    let last = seen.last().unwrap();
    assert_eq!(last.stage, "finishing");
    assert_eq!(last.files_done, plan.items.len());
    assert_eq!(last.bytes_done, last.bytes_total);
    assert!(seen
        .windows(2)
        .all(|pair| pair[0].bytes_done <= pair[1].bytes_done));
    assert!(seen
        .iter()
        .any(|p| p.current.as_deref() == Some("catalog imaging-rig")));
}

#[test]
fn pruning_touches_only_the_schedules_own_archives() {
    let setup = setup();
    let plan = plan(&setup.locations, &setup.registry, BackupScope::Meta, false);
    let destination = setup.root.join("backups");
    std::fs::create_dir(&destination).unwrap();
    let mut scheduled = Vec::new();
    for day in 0..4 {
        let written = write(
            &plan,
            &destination,
            Trigger::Schedule,
            moment(day * 86_400),
            &Hooks::quiet(),
        )
        .unwrap();
        scheduled.push(written.file_name);
    }
    let manual = write(
        &plan,
        &destination,
        Trigger::Manual,
        moment(-86_400),
        &Hooks::quiet(),
    )
    .unwrap()
    .file_name;
    // Named like ours, but no manifest: not ours.
    let impostor = archive_name(moment(-5 * 86_400), BackupScope::Meta, Trigger::Schedule);
    std::fs::write(destination.join(&impostor), "not a zip").unwrap();
    std::fs::write(destination.join("holiday-photos.zip"), "zip").unwrap();
    // Work in progress: an old one from a dead run, and a fresh one.
    let stale = destination.join(format!("{TEMP_PREFIX}dead.zip.partial"));
    std::fs::write(&stale, "partial").unwrap();
    let old = filetime::FileTime::from_unix_time(1_000_000_000, 0);
    filetime::set_file_mtime(&stale, old).unwrap();
    let fresh = destination.join(format!("{TEMP_PREFIX}running.zip.partial"));
    std::fs::write(&fresh, "partial").unwrap();

    let listed = list(&destination);
    assert_eq!(listed.len(), 5, "the impostor is not listed");
    assert_eq!(listed[0].file_name, scheduled[3], "newest first");

    let removed = prune(
        &destination,
        schedule::Retention::Count { count: 2 },
        Some(&destination.join(&scheduled[3])),
        moment(4 * 86_400).timestamp(),
    )
    .unwrap();
    assert_eq!(removed, vec![scheduled[1].clone(), scheduled[0].clone()]);
    for kept in [&scheduled[2], &scheduled[3], &manual, &impostor] {
        assert!(destination.join(kept).exists(), "{kept} was removed");
    }
    assert!(destination.join("holiday-photos.zip").exists());
    assert!(!stale.exists(), "a dead run's partial file is cleared");
    assert!(fresh.exists(), "a running backup's file is left alone");

    // By age: everything older than a day goes, but the newest stays even
    // when it is old too.
    let removed = prune(
        &destination,
        schedule::Retention::Age { days: 1 },
        Some(&destination.join(&scheduled[3])),
        moment(30 * 86_400).timestamp(),
    )
    .unwrap();
    assert_eq!(removed, vec![scheduled[2].clone()]);
    assert!(destination.join(&scheduled[3]).exists());
    assert!(destination.join(&manual).exists());
}

#[test]
fn the_archive_just_written_survives_a_prune() {
    let setup = setup();
    let plan = plan(&setup.locations, &setup.registry, BackupScope::Meta, false);
    let destination = setup.root.join("backups");
    std::fs::create_dir(&destination).unwrap();
    let older = write(
        &plan,
        &destination,
        Trigger::Schedule,
        moment(10),
        &Hooks::quiet(),
    )
    .unwrap();
    // A clock that went back: the new archive looks older than the last.
    let newer = write(
        &plan,
        &destination,
        Trigger::Schedule,
        moment(0),
        &Hooks::quiet(),
    )
    .unwrap();
    prune(
        &destination,
        schedule::Retention::Count { count: 1 },
        Some(&newer.path),
        moment(20).timestamp(),
    )
    .unwrap();
    assert!(newer.path.exists());
    assert!(older.path.exists());
}

#[test]
fn a_folder_beside_a_catalog_or_in_its_images_is_flagged() {
    let setup = setup();
    let none = destination_warnings(&setup.root.join("elsewhere"), &setup.registry, &[]);
    assert!(none.is_empty(), "{none:?}");
    let beside = destination_warnings(&setup.root.join("rig/backups"), &setup.registry, &[]);
    assert_eq!(beside.len(), 1);
    assert!(beside[0].contains("Imaging Rig"));
    let images = destination_warnings(&setup.root.join("images/b"), &setup.registry, &[]);
    assert!(images[0].contains("image folder"));
    let cache = setup.root.join("cache");
    let generated = destination_warnings(
        &cache.join("x"),
        &setup.registry,
        std::slice::from_ref(&cache),
    );
    assert!(generated[0].contains("generated files"));
}

#[test]
fn archive_names_carry_the_time_scope_and_trigger() {
    let created = Utc.with_ymd_and_hms(2026, 10, 16, 13, 0, 5).unwrap();
    assert_eq!(
        archive_name(created, BackupScope::Full, Trigger::Schedule),
        "psf-guard-backup-2026-10-16T130005Z-full-auto.zip"
    );
    assert_eq!(
        archive_name(created, BackupScope::Meta, Trigger::Manual),
        "psf-guard-backup-2026-10-16T130005Z-meta.zip"
    );
    assert!(is_archive_name(
        "psf-guard-backup-2026-10-16T130005Z-meta-2.zip"
    ));
    assert!(!is_archive_name(
        "psf-guard-backup-2026-10-16T130005Z-meta.zip.partial"
    ));
    assert!(!is_archive_name(
        "../psf-guard-backup-2026-10-16T130005Z-meta.zip"
    ));
}

#[test]
fn the_desktop_settings_file_is_where_the_desktop_app_keeps_it() {
    let registry = Path::new("/x/config.json");
    let locations = Locations::beside_registry(registry);
    let desktop = locations
        .meta_files()
        .into_iter()
        .find(|(role, _, _)| *role == FileRole::DesktopSettings)
        .unwrap()
        .2;
    assert_eq!(desktop, crate::desktop_server::Settings::path(registry));
}

#[test]
fn a_custom_registry_name_finds_its_own_siblings() {
    let locations = Locations::beside_registry("/x/registry.json");
    let files: Vec<PathBuf> = locations
        .meta_files()
        .into_iter()
        .map(|(_, _, path)| path)
        .collect();
    assert!(files.contains(&PathBuf::from("/x/registry.auth.json")));
    assert!(files.contains(&PathBuf::from("/x/registry.director-meta.sqlite")));
}

/// A schedule pointed at a removable disk or NAS mount must not write onto
/// the system disk while it is unmounted by creating every missing folder
/// up to the mount point.
#[test]
fn a_folder_on_a_missing_mount_is_not_created() {
    let setup = setup();
    let plan = plan(&setup.locations, &setup.registry, BackupScope::Meta, false);
    let mount = setup.root.join("Volumes").join("BackupDisk");
    let result = write(
        &plan,
        &mount.join("psf-guard"),
        Trigger::Schedule,
        moment(0),
        &Hooks::quiet(),
    );
    let error = format!("{:#}", result.as_ref().unwrap_err());
    assert!(error.contains("is its disk mounted?"), "{error}");
    assert!(
        !mount.exists(),
        "created the mount point and wrote {:?}",
        result.map(|written| written.path)
    );
    // A scheduled run needs its folder to be there; a person's run creates
    // the last folder, but not its parents.
    let missing = setup.root.join("present").join("backups");
    std::fs::create_dir(setup.root.join("present")).unwrap();
    assert!(write(
        &plan,
        &missing,
        Trigger::Schedule,
        moment(0),
        &Hooks::quiet()
    )
    .is_err());
    write(&plan, &missing, Trigger::Manual, moment(0), &Hooks::quiet()).unwrap();
    assert!(write(
        &plan,
        &setup.root.join("absent").join("backups"),
        Trigger::Manual,
        moment(0),
        &Hooks::quiet()
    )
    .is_err());
}

/// Two installs (the desktop app and a server, or two registries)
/// scheduled into one NAS folder must not prune each other's backups: the
/// manifest says which install made each.
#[test]
fn two_installs_sharing_a_folder_keep_their_own_archives() {
    let desktop = setup();
    let server = setup();
    let shared = desktop.root.join("nas");
    std::fs::create_dir(&shared).unwrap();
    let desktop_plan = plan(
        &desktop.locations,
        &desktop.registry,
        BackupScope::Meta,
        false,
    );
    let server_plan = plan(
        &server.locations,
        &server.registry,
        BackupScope::Meta,
        false,
    );
    let mine = write(
        &desktop_plan,
        &shared,
        Trigger::Schedule,
        moment(0),
        &Hooks::quiet(),
    )
    .unwrap();
    let theirs = write(
        &server_plan,
        &shared,
        Trigger::Schedule,
        moment(60),
        &Hooks::quiet(),
    )
    .unwrap();
    prune(
        &shared,
        schedule::Retention::Count { count: 1 },
        Some(&theirs.path),
        moment(120).timestamp(),
    )
    .unwrap();
    assert!(
        mine.path.exists(),
        "the server pruned the desktop app's only backup"
    );
}

/// By hand: run with PSF_GUARD_REVIEW_DIR set to a folder on exFAT, FAT or
/// SMB to check an archive is saved there.
#[cfg(unix)]
#[test]
#[ignore]
fn writes_into_the_folder_named_by_env() {
    let Ok(folder) = std::env::var("PSF_GUARD_REVIEW_DIR") else {
        return;
    };
    let setup = setup();
    let plan = plan(&setup.locations, &setup.registry, BackupScope::Full, false);
    let written = write(
        &plan,
        Path::new(&folder),
        Trigger::Schedule,
        Utc::now(),
        &Hooks::quiet(),
    )
    .unwrap_or_else(|error| panic!("{error:#}"));
    assert!(written.path.exists());
    assert_eq!(
        read_manifest(&written.path).unwrap().trigger,
        Trigger::Schedule
    );
    use std::os::unix::fs::PermissionsExt;
    eprintln!(
        "wrote {} mode {:o}",
        written.path.display(),
        std::fs::metadata(&written.path)
            .unwrap()
            .permissions()
            .mode()
    );
}

/// Where a file system cannot rename without replacing (exFAT answers
/// ENOTSUP), the archive claims its name with an exclusive create and
/// renames over that placeholder, still never over someone else's file.
#[test]
fn the_fallback_save_claims_the_name_and_never_replaces_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("archive.zip");
    let mut file = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
    std::io::Write::write_all(&mut file, b"archive").unwrap();
    persist_over_claim(file, &target).unwrap_or_else(|(_, error)| panic!("{error}"));
    assert_eq!(std::fs::read(&target).unwrap(), b"archive");

    let theirs = dir.path().join("theirs.zip");
    std::fs::write(&theirs, "someone else's").unwrap();
    let file = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
    let (back, error) = persist_over_claim(file, &theirs).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert!(
        back.path().exists(),
        "the archive is handed back to try another name"
    );
    assert_eq!(std::fs::read_to_string(&theirs).unwrap(), "someone else's");
}

#[cfg(unix)]
#[test]
fn an_unsupported_no_replace_rename_is_recognised() {
    assert!(no_replace_unsupported(&std::io::Error::from_raw_os_error(
        libc::ENOTSUP
    )));
    assert!(no_replace_unsupported(&std::io::Error::from_raw_os_error(
        libc::EINVAL
    )));
    assert!(!no_replace_unsupported(&std::io::Error::from_raw_os_error(
        libc::EEXIST
    )));
    assert!(!no_replace_unsupported(&std::io::Error::from_raw_os_error(
        libc::EACCES
    )));
}

/// A scheduled run that could not reach a catalog keeps the older archive
/// that holds it, whatever the retention says, and says what it missed.
#[test]
fn an_archive_holding_a_catalog_the_new_one_lacks_is_kept() {
    let setup = setup();
    let destination = setup.root.join("scheduled");
    std::fs::create_dir(&destination).unwrap();
    let full = plan(&setup.locations, &setup.registry, BackupScope::Full, false);
    let complete = write(
        &full,
        &destination,
        Trigger::Schedule,
        moment(0),
        &Hooks::quiet(),
    )
    .unwrap();
    std::fs::rename(
        setup.root.join("rig").join("schedulerdb.sqlite"),
        setup.root.join("rig").join("away.sqlite"),
    )
    .unwrap();
    let partial_plan = plan(&setup.locations, &setup.registry, BackupScope::Full, false);
    let partial = write(
        &partial_plan,
        &destination,
        Trigger::Schedule,
        moment(86_400),
        &Hooks::quiet(),
    )
    .unwrap();
    assert!(partial
        .manifest
        .problems
        .iter()
        .any(|problem| problem.contains("catalog imaging-rig")));
    let removed = prune(
        &destination,
        schedule::Retention::Count { count: 1 },
        Some(&partial.path),
        moment(86_400).timestamp(),
    )
    .unwrap();
    assert!(removed.is_empty(), "{removed:?}");
    assert!(complete.path.exists());
}
