# Calibrated subs

Status: **Design; being built in three PRs (see the end)**
Last updated: 2026-10-04

## 1. Goal

People calibrate and register their lights in other applications: PixInsight
(WBPP writes `_c.xisf`, `_c_cc.xisf`, `_r.xisf`), Siril (`pp_` and `r_`
prefixes), Astro Pixel Processor, and others. PSF Guard should know those
files for what they are:

- a calibrated or registered copy of a light it already catalogs is
  **paired** with that light, not catalogued as a second one;
- a catalog that holds **only** calibrated subs works as well as one with
  raw frames;
- a pair moves, grades and archives as one frame, and the calibrated copy
  can be viewed and measured.

Generating calibrated frames in PSF Guard is out of scope here; the side
table below is shaped so a later pass can record frames PSF Guard writes.

### What happens today

`image_io::is_processing_artifact` recognises integrations (`NCOMBINE`,
master `IMAGETYP`, `SEIZAMST`) and anything PixInsight's pipeline wrote
(XISF processing history and signatures). Import consults it through
`ImportOptions::skip_processed`, which **defaults to off**, so auto-import,
remote upload and folder imports catalog a `_c.xisf` as a new light. Siril
and APP output has no rule at all. The five catalogs on the maintainer's
server hold no such rows today, so the new rules apply from a clean start.

## 2. Classifying a frame

`image_io` gains a classification beside `is_processing_artifact`:

| Kind | Meaning |
|---|---|
| `raw` | An acquisition as the camera wrote it. |
| `calibrated` | Bias, dark and/or flat applied; geometry unchanged. |
| `registered` | Resampled onto another frame's grid (implies calibrated in practice). |
| `integration` | A master or a stack: never a light. |

Evidence, strongest first; the first that decides wins:

1. **Integration marks**: `NCOMBINE` (PixInsight, DeepSkyStacker), master
   `IMAGETYP`, `SEIZAMST`, a frame count above one (Siril `STACKCNT`, APP
   `NUMFRAME` beside `INTEGRAT`, ASTAP `LIGH_CNT` and per-filter counts,
   MaxIm `SNAPSHOT`), ASTAP's `S` in `CALSTAT`.
2. **Processing history the tool wrote**, read through
   `image_io::read_frame_header` (seiza-fits keeps the `HISTORY` and
   `COMMENT` text since 0.2.5):
   - PixInsight XISF: `PixInsight:ProcessingHistory` process classes
     (`StarAlignment` registered; `ImageCalibration`, `CosmeticCorrection`,
     `Debayer` calibrated; `ImageIntegration`, `DrizzleIntegration`,
     `FastIntegration` integration) and `PCL:Signature:*` properties. A file
     PixInsight marked with none of these is called calibrated, so it still
     stays out of the lights.
   - Siril `HISTORY`: "Calibrated with a master dark/bias/flat", "Cosmetic
     correction of …", stacking parameters; `Crop (…)` and `Rotation (…)`
     change the geometry and count as registered.
   - ASTAP: `COMMENT 1  Calibrated by ASTAP` and `… Calibrated & aligned by
     ASTAP`; other comments are free text and ignored.
   - `CALSTAT` with B, D or F (MaxIm, ASTAP), `PEDESTAL` (MaxIm 100, ASTAP
     500), ASTAP's `DARK_CNT`, `FLAT_CNT`, `BIAS_CNT`.
3. **File names**, only on floating-point samples: WBPP suffixes `_c`,
   `_cc`, `_d`, `_r` in any run; ASTAP `_cal`, `_aligned`; DeepSkyStacker
   `.cal` and `.reg`; Siril prefixes `pp_`, `bkg_` (calibrated), `r_`,
   `cropped_` (registered), which stack (`r_bkg_pp_`); a `stacked` ending is
   an integration. Calibration output is float and camera data is integer,
   so a raw `M31_r.fits` shot through an r filter stays raw.
4. Otherwise `raw`.

Software names alone decide nothing: Siril stamps `PROGRAM` on any file it
saves, and APP copies N.I.N.A.'s `SWCREATE` forward. They only name the
`producer` once something else has decided the kind.

Siril's registration writes no mark at all (the transforms live in its
`.seq` file), so an `r_` frame is known by its name alone.

The classification, its evidence (`header` or `name`) and producer travel
with the frame, so a report can say why a frame was called calibrated.

## 3. Pairing a derivative with its raw light

Calibration tools copy the acquisition headers forward, so a derivative
still carries the raw's `DATE-OBS`, exposure, filter, object and camera.
A derivative pairs with the catalogued light where all of these hold:

- `DATE-OBS` equal to the second (sub-second where both record it);
- exposure equal within 1 ms;
- filter equal after the catalog's filter-name normalisation;
- the same target, by `OBJECT` or by the light's target when `OBJECT` is
  absent, and the same camera where both name one.

The file stem settles a tie (`frame_0042_c.xisf` against
`frame_0042.fits`). A derivative that matches two lights and no name
settles it is **ambiguous**: it is reported and not paired.

A derivative known only by its **name** pairs only with a light that its
name or its recorded source names. This is not caution for its own sake: on
the maintainer's share, Siril registered an APP stack, and the result has no
stack count and carries the reference light's time, exposure and filter.
Matching keywords alone would pair that stack with the light. Siril renames
frames when it builds a sequence, so its `<sequence>_conversion.txt`
(`'source' -> 'frame'` lines) is the record of where each frame came from;
when that source is itself an integration, every derivative of it is one too.

When several copies of one kind match one light (WBPP left both `_r` and
`_c_r` of the same frame on the C925 run), the one that was calibrated
wins, then the newer file; the rest are reported as superseded.

Pairing is **on by default**:

- import routes a derivative into pairing instead of cataloguing it;
- a background pass after each directory refresh pairs derivatives already
  on disk, off the request path, the way the calibration header backfill
  runs (own connection, chunked commits, one pass at a time per catalog);
- a database setting turns it off, for someone who wants their calibrated
  files left alone.

## 4. Catalogs with only calibrated subs

A derivative with no raw light to pair with becomes a light row of its own,
because the catalog, grading and stacking all need one. Its side-table record
marks it `primary`: the row's file *is* the derivative.

When the raw arrives later, by import or sync, import pairs it with that row
instead of adding a twin: the row keeps its id, GUID, grade and history; its
`FileName` metadata moves to the raw; and the derivative's record stops being
primary. This is the one place PSF Guard rewrites a light's file name, and
only on a row it marked itself.

Quality screening treats a primary calibrated light as calibrated: the
statistics that assume raw ADU (sky level against the zero point, the
sensor-temperature limits) skip or renormalise rather than punish it.

## 5. The side table

`psf_guard_frame_derivative`, in the scheduler database beside the other
PSF Guard tables; Target Scheduler tables are never altered.

| Column | Meaning |
|---|---|
| `derivative_uuid` TEXT PK | Stable identity, the sync key. |
| `acquired_image_guid` TEXT | The light it belongs to (`acquiredimage.guid`). |
| `kind` TEXT | `calibrated` or `registered`. |
| `primary_source` INTEGER | 1 when the light row's own file is this derivative. |
| `file_name` TEXT | Basename; resolved locally through the directory tree. |
| `source_tail` TEXT | The last path segments where it was found, for the resolver. |
| `size`, `mtime` INTEGER | The fingerprint the record was made from. |
| `producer` TEXT | `pixinsight`, `siril`, `app`, `dss`, `astap`, `maxim`, `unknown` (`psf-guard` later). |
| `evidence` TEXT | `header` or `name`. |
| `created_at`, `updated_at` INTEGER | Epoch seconds. |

One record per (light, kind): a light has at most one calibrated and one
registered copy. A newer file of the same kind replaces the record. The
table has its own version row (`psf_guard_frame_derivative_schema`); later
changes follow the calibration library's ladder and backup, and a build
refuses to write a table a newer build has upgraded.

Paths are not stored absolutely: like calibration sources, a record is found
again by basename and tail through the local directory tree, checked against
its fingerprint, so the same record works on every machine that syncs it.

## 6. Sync

The table splits the catalog further from Target Scheduler, so sync must
carry it explicitly:

- **Pull** (`sync_pull_in_transaction`): after `upsert_acquired_images`,
  upsert records by `derivative_uuid` and remap `acquired_image_guid` through
  the image map, the way `calibration::sync_library` carries the library.
  A record whose light did not come across is skipped and counted.
- **Remote HTTP sync**: add the table to the bundle's merge set and apply it
  in the same step.
- **Grade push** needs nothing: grades live on the light.
- A destination that already has a primary calibrated row and receives the
  raw from the source pairs them (section 4) rather than inserting a twin,
  an extension of the existing rule that sync updates a row PSF Guard minted
  itself.

## 7. Behaviour of a pair

- **Grades** live on the light. The derivative has none of its own.
- **Reject archive**: `move-rejects` moves a light's derivatives with it,
  each into the archive mirror of its own folder, recorded beside the
  sidecars; `restore-rejects` brings them back. Dry runs list them.
- **Previews and inspection**: the detail view gets a Raw / Calibrated /
  Registered switch where copies exist. Previews, annotated previews, star
  and PSF caches take a `source` parameter whose cache-key segment includes
  the derivative's fingerprint, so raw keys stay where they are.
- **Library and grid**: a light with a calibrated copy carries a small mark;
  a primary calibrated light says so.
- **Quality scans** measure the raw by default. A database option measures
  the calibrated copy instead, for its flat-fielded background and its
  hot-pixel-free stars. Its scan entries carry a `calibrated:` source
  revision, so raw and calibrated measurements never stand in for each
  other, and its star counts are not written into the light's metadata,
  which keeps the capture software's meaning. A **registered** copy never
  feeds HFR, star, pointing or astrometry evidence: resampling changes all
  of them.
- **Stacking** reads raw frames and calibrates them as today. Using
  supplied calibrated frames as stack input is a later option.

## 8. Delivery

1. **Engine**: classification, the side table and its schema, the pairing
   matcher, and this record. No behaviour changes yet.
2. **Import and sync**: derivatives go to pairing on import and in the
   background pass, calibrated-only catalogs import, the raw-arrives-later
   case, pull and remote sync, the database setting.
3. **Using pairs**: the detail switch and marks, previews by source, the
   reject archive, and the quality-scan option.
