# Adding FITS folders

PSF Guard can start from an existing N.I.N.A. Target Scheduler database or
build a compatible image catalog from folders of FITS light frames. The first
import reads FITS headers only. It does not decode every image or run quality
analysis, so a large library becomes usable without waiting for star
detection, photometry, or plate solving.

## Create a catalog in the UI

The desktop app always allows database management. For the web server, bind to
a trusted interface and opt in:

```bash
psf-guard server --host 127.0.0.1 --allow-database-management
```

Then open **Settings** and choose **New Database from Images**.

![New Database from Images form](import-from-images.png)

1. Enter a display name. If you leave it blank, PSF Guard uses **Imported
   Images**.
2. Add one or more absolute paths. Each path must exist and must be a directory;
   an invalid path stays in the form with the missing path named in the error.
3. Leave **Queue background quality analysis** off for a quick first import.
4. Choose **Create & Import**. The settings panel shows header-scan and import
   progress. If you close or reload the page, opening Settings finds the active
   server job and resumes the progress view.

The UI stores the new catalog as a SQLite file under `databases/` beside the PSF Guard
registry. Its name comes from the display name, with `-2`, `-3`, and so on added
when needed. The database row in Settings shows the exact path. The CLI form
creates the database at the path you give it instead:

```bash
psf-guard create-db ./archive.sqlite /data/lights /data/more-lights \
  --name "Archive"
```

`create-db --dry-run` creates a temporary schema, previews the plan, and then
removes the new file. Add `--no-register` if you do not want the result added to
the shared registry.

## What the header import builds

PSF Guard writes a full Target Scheduler-compatible schema. This inherited
mapping lets it open an existing Scheduler database directly, but Target
Scheduler is not required. The import creates:

- one profile for the imported rig;
- projects and targets;
- shared exposure templates;
- exposure plans; and
- acquired-image rows, initially graded Pending.

The grouping rules stay conservative:

- FITS `OBJECT` names define targets. Frames from different telescope, camera,
  focal length, or binning signatures never share a project.
- Each target gets its own project by default, even when its frames span many
  nights.
- Panel-style targets share a mosaic project only when their names have the
  same panel root, their centers are within five degrees, and their capture
  dates are within 14 days by default.
- Target coordinates use the median FITS coordinates. Target Scheduler stores
  RA as decimal hours and Dec as degrees.
- Filter, gain, offset, binning, and numeric readout mode define a shared
  exposure template. The most common exposure length for that template becomes
  its default.
- Each distinct template and exposure length becomes an exposure plan. The
  imported frame count seeds its desired and acquired counts.

![The shared exposure-template view for a real narrowband project](project-plan-narrowband.png)

![The target-coordinate and exposure-plan view for the same narrowband project](target-plan-narrowband.png)

The import does not plate-solve folders or verify the `OBJECT` name against the
pixels. Run quality analysis later to add fresh star measurements, spatial and
photometric evidence, and pixel-derived pointing checks. That keeps a bad or
missing solver catalog from blocking the initial catalog build.

## Add new frames later

Use the **Import** button on a configured database. PSF Guard first runs a dry
preview and lists frames that will attach to existing targets and frames that
will create new projects. Confirm the preview to write it.

The matching rules prevent common duplicates and splits:

- a basename already stored in the database is skipped;
- an `OBJECT` name match attaches to the existing target;
- otherwise, coordinates within 0.5 degrees attach to the existing target; and
- unmatched frames create new project and target rows.

The CLI exposes the same path:

```bash
psf-guard import archive ./new-lights --dry-run
psf-guard import archive ./new-lights
```

Use `--no-attach` when every new frame should create imported structure instead
of attaching to an existing name or coordinate match. `remove-imported` can
remove projects created by an import; always preview it first.

## Fill or refresh quality data

Header import leaves pixel work for a separate, low-priority server job. Each
database card in Settings has two actions:

- **Analyze Missing Quality** keeps valid cached work and fills gaps.
- **Rescan All Quality** recomputes star counts and all cached quality evidence.

Both actions process targets in the background, show their current target and
progress, and yield to interactive preview and scan work. The settings page
reads the server's job state when it opens, so navigation or a page reload does
not hide a running job. A server restart ends the in-memory job; start it again
to resume from the persistent cache.

![Settings showing Analyze Missing Quality and Rescan All Quality beside the Seiza catalog controls](settings-catalog-quality.png)

The backfill uses the N.I.N.A. Fast detector for star count and HFR, which keeps
new values comparable with Target Scheduler data. It also stores the full-star
flux data needed by photometry, spatial cloud and obstruction metrics, and
fresh plate-solving evidence for off-target and tracking checks.

## Review and correct the imported plan

Open **Overview**, then choose **Plan & coordinates** on a project. You can see
project state and limits, target coordinates, rotation and ROI, shared exposure
templates, and each filter plan. With database management enabled, you can edit
these fields, add plans, or merge and move imported projects and targets when a
header name grouped them poorly.

## Sync with the telescope database

Add both the local and telescope databases in Settings, then expand
**Scheduler database sync**:

- **Preview full pull** copies new or changed projects, targets, templates,
  plans, captures, and grades from the telescope database into the local one.
  Local reviewed grades win.
- **Preview planning push** sends project, target, template, plan, and rule
  settings back to the telescope database. It does not change telescope image
  rows, capture counts, or grades.

Both actions show a dry preview before **Apply**. The CLI equivalents are:

```bash
psf-guard sync pull --from telescope.sqlite --to archive.sqlite --dry-run
psf-guard sync planning --from archive.sqlite --to telescope.sqlite --dry-run
psf-guard sync grades --from archive.sqlite --to telescope.sqlite --dry-run
```

Back up both databases before the first write.
