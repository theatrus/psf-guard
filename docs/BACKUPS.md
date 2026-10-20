# Backups

PSF Guard can back up its own settings and the catalogs it works on, now or
on a schedule. Open **Settings › Backups**, or run `psf-guard backup`.

Backups need an editor on a server that allows database management: the
desktop app, or a server started with `--allow-database-management`. An
archive holds every catalog and the server's settings, so a read-only
account cannot start, list or download one.

## What a backup holds

One zip archive, named `psf-guard-backup-<UTC time>-<scope>.zip` (`-auto`
for a scheduled one). It holds:

- `manifest.json`: the PSF Guard version, when and how the backup was made,
  every file with its SHA-256, size and source path, and, for each SQLite
  file, its Target Scheduler schema (`user_version`), PSF Guard's own schema
  version, a `quick_check` result and its `psf_guard_*` tables;
- `README.txt`: how to restore by hand;
- `meta/`: PSF Guard's own state (below);
- `catalogs/<slug>/`: for a full backup, each registered catalog.

There are two scopes:

| Scope | Holds | Size on a scratch setup |
|---|---|---|
| Settings only (`meta`) | PSF Guard's own files | 702 KB, mostly Planning's store |
| Settings and catalogs (`full`) | The same, plus every catalog | 731 MB before compression for two catalogs, one with a million frames |

### PSF Guard's own state

These sit beside the registry, in `~/Library/Application Support/psf-guard/`
on macOS, `~/.config/psf-guard/` on Linux and `%APPDATA%\psf-guard\` on
Windows. A registry given with `--registry other.json` has its own files,
named `other.auth.json` and so on.

| File | What it is |
|---|---|
| `config.json` | The registry: every catalog, its image folders, and the settings Settings saves (calibration, stacking, storage, export, peers, the backup schedule) |
| `auth.json` | Browser accounts: password hashes and API token hashes |
| `user-preferences.json` | Each account's view preferences |
| `processing-setups.json` | Saved processing setups |
| `collaboration-credentials.json` | Collaboration service tokens |
| `desktop.json` | The desktop app's server port |
| `director-meta.sqlite` | Planning's store: rigs, sites, plans and pairings |
| The server's TOML file | When the server started with `--config` |
| `remote-sync-audit.jsonl` | The record of remote sync actions, in the cache folder |

### Catalogs

Each registered Target Scheduler catalog, with the `psf_guard_*` tables PSF
Guard adds to it: grades and reject reasons, calibration frames and masters'
records, flat measurements, the reject archive's records and the rest.

SQLite files are copied with SQLite's online backup, never by copying the
file. The source opens read-only. The copy runs 1 MiB at a time with a short
pause between steps, so N.I.N.A. and the server keep writing while it runs.
A write makes SQLite start that file again, and after a few restarts the
steps grow so a busy catalog still finishes. A catalog in WAL mode never
blocks a writer, so its last step may take the rest of the file at once. A
catalog with a rollback journal blocks writers during each step, so its
steps stay at 16 MiB or less, and one that keeps changing through 40
restarts is left out of that backup rather than locked. A catalog that cannot be
reached is listed in the manifest and left out; the rest of the backup goes
on.

### What a backup leaves out

PSF Guard makes these again, so a backup never holds them:

- image previews, annotated previews and star lists (the cache folder);
- stack previews and their processing (the stacks folder);
- calibration masters (the calibration folder);
- astrometry solves, satellite elements and sky maps (the cache folder);
- Seiza star catalogs, which Settings › Catalogs installs again;
- the FITS and XISF frames themselves.

On a working rig these folders are usually far larger than everything a
backup holds: Settings › Storage shows their sizes.

### Secrets

PSF Guard keeps some credentials in the clear: outgoing peer keys in the
registry, collaboration service tokens, and any `token` written in the
server's TOML file. **Include secrets** starts on for manual desktop backups
and new desktop schedules. Saved schedules keep their existing choice. It
starts off in a browser, and the CLI requires `--include-secrets`. When
secrets are excluded:

- peer keys are blanked;
- the collaboration credentials file is left out whole;
- TOML tokens become `REDACTED`, which is too short to be a valid token, so
  a restored server says what is missing rather than accepting a known key.

Account password hashes, API token hashes and remote upload key hashes are
kept either way: they are what PSF Guard itself stores, and a restore
without them would lose every account. The manifest says whether secrets are
in the archive and what was removed.

Secrets go only into a server folder, at a person's request: someone signed
in to the server, or the desktop app. An API token can neither ask for them
nor schedule them. An archive that holds them is never offered as a
download, to anyone, nor is one whose manifest cannot be read: it stays on
the server's disk, and the Backups list says so in place of the Download
link.

On a local macOS or Linux disk, archives are written readable only by their
owner. FAT, exFAT and SMB shares do not keep those permissions, and on
Windows an archive takes its folder's: keep an archive with secrets in a
folder only you can read.

## Back up now

Choose the scope and whether to include secrets, then **Back up now**. In a
browser, leave the folder empty to download the archive; the server keeps
it in `<cache>/backup-downloads/` for a day, and sweeps older ones every 30
seconds and at start. In the desktop app, choose a folder with **Browse…**.
PSF Guard suggests `~/Documents/psf-guard/backups` and selects **Include
secrets** by default. It creates the suggested folder and any missing parent
folders. For another destination, its parent must already exist; a missing
parent, such as an unmounted disk, fails the backup. Progress appears in
Settings and the header's job list, with a Stop button.

## Scheduled backups

Turn on **Back up on a schedule** and choose:

- **Every** day or week, the weekday, and the time of day in the server's
  local time. Midday is the default: imaging runs at night, so catalogs are
  quiet by day.
- **Folder**: required; `~/Documents/psf-guard/backups` is suggested.
  Saving creates the folder. For another destination, the parent folder
  must already exist, and a scheduled run fails if the destination later
  goes missing. PSF Guard recreates the suggested folder when needed.
  Settings warns when the destination is inside a catalog's folder, an
  image folder, or PSF Guard's generated folders.
- **What to back up**: settings and catalogs by default.
- **Include secrets**: on by default for a new desktop schedule, off in a
  browser. Saved schedules keep their existing choice. Archives with
  secrets cannot be downloaded.
- **How many to keep**: the newest 7 by default, or every one younger than a
  number of days, the newest always kept.

Scheduled backups are off until you turn them on. A run is a background job:
it waits while work you started runs, such as a stack build, and the job
line lists it as waiting meanwhile. A server that was off at the scheduled
time runs once when it comes back. A run that fails is tried again an hour
later; a stopped run waits for the next scheduled time. These retry and stop
rules persist across restarts, as does the last backup's result. PSF Guard
saves them in `backup-state.json` beside the registry. You can also stop a
run while it waits.

After a complete scheduled backup, PSF Guard deletes older archives according
to the rule you chose, with these safeguards:

- It deletes only scheduled archives made by this installation. It checks both
  the file name and the manifest. Manual backups, backups from another
  installation and other files stay unchanged. Each archive records the
  installation id created when you save the schedule.
- It keeps older archives that contain a catalog missing from the new backup.
  If a catalog stays unreachable, for example on an unplugged disk, PSF Guard
  still removes older backups of the available catalogs. It keeps backups of
  the missing catalog, and Settings shows "Kept N older backups because they
  hold <catalog>, which this run left out." If you change the schedule to back
  up settings only, remove older full backups by hand when you no longer need them.
- If PSF Guard cannot copy a settings file, it reports the error and deletes
  no older archives.

Every run also clears work-in-progress files a crashed run left in its
folder over a day ago.

Archives are saved under a temporary name and renamed into place without
replacing any file. On a file system that cannot rename without replacing,
such as exFAT, the archive first claims its name with a new empty file and
then replaces that.

Settings shows the next run and the last backup result, including after a
restart. It lists archives in the schedule's folder and the download area,
with browser download links for archives without secrets.

## From the command line

```bash
psf-guard backup /mnt/backups                      # settings and catalogs
psf-guard backup /mnt/backups --scope meta         # settings only
psf-guard backup /mnt/backups --dry-run            # say what it would copy
psf-guard backup /mnt/backups --include-secrets
psf-guard backup /mnt/backups --registry /etc/psf-guard/config.json \
  --config /etc/psf-guard/psf-guard.toml
```

It uses the same code as Settings and may run while the server runs.

## Restoring by hand

PSF Guard does not restore a backup itself: a wrong restore could replace a
catalog N.I.N.A. is using. To restore:

1. Stop PSF Guard: quit the desktop app, or stop the server. If you restore
   a catalog that is N.I.N.A.'s scheduler database, close N.I.N.A. too.
2. Unzip the archive somewhere new. Check a file against `manifest.json`
   with `shasum -a 256 <file>` if you want to be sure of it.
3. Keep a copy of every file you are about to replace.
4. Copy each file under `meta/` back to its `source` path in
   `manifest.json`. For a full backup, copy each file under
   `catalogs/<slug>/` back to its catalog's `db_path`. Delete any
   `-wal` and `-shm` files beside a catalog you replace.
5. If the backup was made without secrets, enter peer keys, collaboration
   sign-ins and the server's remote tokens again.
6. Start PSF Guard. Previews, stacks and masters are made again as you use
   them.

To move to a new machine, restore the same way and then fix catalog and
image folder paths in Settings › Databases if they differ.

## Engineering notes

- Core: `src/backup/` (plan, archive, manifest, listing, pruning, schedule,
  secrets, online copy).
- Server job and routes: `src/server/backup_job.rs`, under `/api/backups`.
  One run at a time, server-wide, on one `CatalogJobs` runner keyed by the
  server's backup manager; the handler only queues it. Stop requests the
  run's `Stop`, and a run queued but not yet started begins stopped. It follows [the background job contract](design/background-jobs.md):
  a scheduled run is `Background` work that yields through
  `AppState::wait_for_interactive_work_unless`, so Stop also ends a wait,
  holds one background worker lease
  while it compresses, and holds the server's stop until it has cleaned up.
- CLI: `src/commands/backup.rs`.
- UI: `static/src/components/BackupSettings.tsx`, and the job line row in
  `static/src/components/header/activityItems.ts`.
