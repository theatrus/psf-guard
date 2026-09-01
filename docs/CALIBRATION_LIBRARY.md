# Calibration libraries

PSF Guard catalogs bias, dark, dark-flat, and flat FITS files beside the Target
Scheduler data. The records live in namespaced tables in the same SQLite file.
The raw frames and generated masters stay on disk.

## Add calibration frames

Add folders that contain calibration files to a database in Settings, then run
**Import**. The dry preview reports light and calibration changes before it
writes anything. `create-db` and `import` use the same header-only scan.

The import recognizes ordinary `IMAGETYP` values:

- `BIAS` or `OFFSET`
- `DARK`
- `DARK FLAT`, `DARKFLAT`, or `FLATDARK`
- `FLAT`

Lights still become Target Scheduler-compatible projects, targets, plans, and
`acquiredimage` rows. Calibration frames do not. They enter these PSF
Guard-owned tables:

- `psf_guard_calibration_schema`
- `psf_guard_rig`
- `psf_guard_rig_binding`
- `psf_guard_calibration_frame`
- `psf_guard_calibration_master`

The catalog stores paths, file fingerprints, capture settings, and provenance.
It does not store FITS pixels or master blobs in SQLite. Reading an untouched
database does not create these tables, and a dry run rolls table creation back.

## Rig identity

A stable rig UUID comes from the telescope, camera, sensor dimensions, and
binning signature. A separate binding records each N.I.N.A. profile ID seen
with that rig. Target Scheduler row IDs are not used as calibration identity.

Settings shows each rig and its bias, dark, dark-flat, and flat coverage. A
database copy carries this catalog with its projects and grades.

Choose **Manage** on a database's calibration summary to inspect every frame.
The library view filters by rig and frame type, marks paths that no longer
exist, and shows the settings used for matching. With database management
enabled, it can:

- scan the database's configured folders again;
- forget a wrong or stale catalog entry without deleting its FITS file;
- clear generated masters while keeping all raw frames and catalog records.

Forgetting a frame also drops every master record that used it, including
downstream masters that used one of those masters. Clearing masters waits for
any active stack preview and rebuilds them on demand later.

## Safe matching

PSF Guard only uses candidates that agree with every known hard setting:

| Master | Required match |
|---|---|
| Bias | camera, dimensions, channels, binning, gain, offset, readout mode, Bayer layout |
| Dark | bias fields, exposure within 0.05 seconds, temperature within 3 C |
| Dark-flat | flat sensor settings, exposure, and temperature |
| Flat | bias fields, filter, telescope, and focal length within 1 mm |

If the light records a hard setting and a candidate omits it, that candidate
does not match. If the light itself omits a setting, PSF Guard cannot use that
field as a gate. The readout mode counts whichever way the camera driver
spelled it: N.I.N.A. writes a name such as `Extend Fullwell 2CMS` rather than
a number, and PSF Guard keeps that name and compares it case-insensitively,
so a High Gain dark never serves an Extend Fullwell light. Opening a library
built before names were kept reads them off the frames' headers once. A match still needs a positive camera-name or sensor-size
identity; wholly unknown sensors never match. It sorts safe candidates by
distance from the light's capture time and uses at most 64 frames per master.

The frames feeding one master must also cohere with each other, which the
build enforces after selection so per-light selections stay identical across
a stack group: candidates cluster by sensor temperature (within 1 °C, the
same gate the stacker itself enforces), and flats also cluster into one
session (within a day of each other — dust moves between sessions). The
nearest cluster with enough frames builds the master, so a stray single flat
near the lights cannot orphan a complete session from a week earlier.

### Validity boundaries

Nearest-in-time is the right default, but it cannot know about a dust
cleaning, a re-spaced imaging train, or any other change that makes older
calibration wrong for newer lights. When nights lack their own frames, mark
the boundary yourself: in the **Calibration library**, frames are grouped by
imaging night, collapsed to one row per night until expanded, with flats
(and their dark-flats) in one section and darks and bias in their own —
those batches stay valid far longer than flats and should not interleave
with per-night flat groups. Each night row can also **Forget night**,
removing that section's records for the night (and dependent masters) in
one step without touching the FITS files. Select a night (or several) and
mark those frames usable
**only for lights after them** (a set shot right after the change) or **only
for lights before them** (the final set shot before it). Marked frames carry
a badge, and matching never selects them across their boundary, in either
direction, for any kind. Marking the same selection **in both directions
again** clears the boundary. A light captured exactly at the boundary
instant matches either way, and a frame with no recorded capture time
cannot be judged and keeps matching — the same rule as any unrecorded
setting. Marks survive folder re-scans, sync to other catalogs with the
rest of the library, and immediately change which masters the next stack
selects.

A stack group whose lights need different master sets — a multi-night
target with per-night flats, or a library large enough that the selection
horizon moves — partitions into calibration sessions. Each light calibrates
with its own session's masters; the stack swaps masters between sessions as
it integrates. The stack card reports the session count, the resume
checkpoint and source-frame search compare the composed per-session
identity, and the search reuses each session's recorded fitted pedestal
rather than fitting its own from the searched subset.

A flat needs the light's pedestal removed before division: dividing a flat
into a light that still carries its pedestal amplifies that pedestal at the
vignetted edges, inverting the vignette into bright edges. With a bias or
dark master present, that master removes the pedestal. With a flats-only
library, PSF Guard fits the pedestal from the lights themselves: the
background is `sky × flat response + pedestal`, so a robust per-tile line
fit against the normalized flat recovers the pedestal as the intercept.
When the fit passes its guardrails — mono frames, enough vignette to give
the fit a lever arm, agreement across sampled lights, and consistency with
the camera's recorded offset for known camera families (ZWO records its
offset in 10 ADU steps) — the stack subtracts the estimate and applies the
flat, the warning states the fitted value, and the masters signature records
it. When any guardrail fails, the stack proceeds without the flat and the
warning says to import bias or dark frames; the built flat master stays
cached and applies as soon as they arrive. The stack panel's **Calibration**
control can override this: **Force on** applies every master that can be
built, including a lone flat with no pedestal estimate — the warning then
says what to expect — and **Off** stacks the raw frames with no matching at
all.

A flat master also gets a spatial defect pass after integration: a hot or
dead sensor pixel repeats identically in every flat, so the across-frame
clipping keeps it by construction, and it would divide a fixed-pattern
artifact into every light. Samples far outside their same-plane
neighborhood are replaced with the neighborhood median; the flat's true
response is smooth at pixel scale, so dust shadows and vignette structure
are untouched. Dark and dark-flat masters keep their hot pixels — they are
what subtracts them from the frames they calibrate.

The defect pass removes pixel-scale impulses, not star images. Stars in
sky flats are handled by the across-frame clipping instead, and only when
they move between exposures — let the sky drift or dither between sky
flats. Sky flats taken with tracking on hold each star on the same pixels
in every frame, and a star that survives into the master is wider than
the defect pass can remove. When no dark master
exists anywhere in a stack's plan, the stack instead runs the same impulse
filter over each calibrated light, and the card says so.

A master that fails to build does not fail the stack. The frames integrate
without that master, and the stack card's calibration warning names the
master that was skipped and the reason. A master whose input master failed —
a flat whose bias did not build — is skipped rather than built without it,
because a flat normalized with the bias pedestal still in it would miscorrect
every light. Resume checkpoints and source-frame searches compare the masters
actually applied, not just the selection, so a build that fails or recovers
between runs forces a clean rebuild instead of mixing calibrations.

## Project coverage report

Each project card on the Overview has a **Calibration** action that reports
how the library covers that project's lights: per kind, how many frames
match and which capture sessions they span; and per imaging night and
filter, the flat session a master would build from, its distance from the
lights, and whether the night has its own flats. One representative light
per night and filter is matched exactly as a stack build would match it, so
the report describes what WOULD apply, not just what files exist. Warnings
call out kinds with no matches, nights without same-night flats, and flats
more than a month from their lights.

## Stack previews

Stack previews build masters on demand with `seiza-stacking`. Each master needs
at least two inputs. Seiza uses a two-pass, leave-one-out sigma-clipped mean and
writes the clipping and source-count provenance into the master FITS.

Generated masters live below:

```text
<cache>/<database>/calibration-masters/
```

The master filename is content-addressed by the source-frame UUIDs, current
file fingerprints, input master UUIDs, algorithm version, and master-cache
version. A changed bias therefore invalidates dark and flat masters that used
it; a changed dark-flat invalidates its flat master. A later stack reuses a
valid file. The database records its source set, input masters, and Seiza
version.

The stack card reports `Calibration applied`, `Calibration set incomplete`, or
no calibration. It also shows input counts and any missing-file warning.
Calibration happens on raw CFA samples before debayering.

One stack uses one master set. If its lights need different sets, PSF Guard
leaves that preview uncalibrated and explains why instead of applying the
reference light's set to every frame.

## Export

Project and target exports add the raw frames matched to each selected light.
Two layouts are available; the default is unchanged.

**Grouped by target**, the default:

```text
BIAS/
DARK/<exposure>_G<gain>/
DARKFLAT/<exposure>_G<gain>/
<target>/FLAT/<filter>/
<target>/LIGHT/<filter>/
```

**WBPP**, one root per frame type, for WeightedBatchPreprocessing:

```text
bias/G<gain>/
darks/<exposure>s_G<gain>/
flats/<target>/<filter>/
lights/<target>/<filter>/
```

Dark flats land in `darks/` beside the lights' darks. WBPP has no dark-flat
type: a dark flat is a dark it pairs to a flat by exposure, so a separate
folder would only mean adding one to WBPP twice.

Flats stay under their target because PSF Guard matched them to that target's
lights. Two targets shot on different nights can need different flats for one
filter, and merging them would have WBPP integrate both into a single master.

Choose the layout with `--layout wbpp` on the CLI, the `layout` query parameter
on the export download, or in the dialog every Export action on the overview
opens. The dialog starts from the **Export** default in Settings → Setups,
which is server-wide and shared by the desktop and browser apps.

A WBPP export also carries `run-wbpp.sh` and `run-wbpp.cmd`, which hand it to
PixInsight. WBPP 3.x is driven from PixInsight's command line rather than
through a script API, so these are the invocation itself: readable, editable,
and re-runnable. They default to WBPP's `loadOnly`, which loads the frames and
groups and then stops with the dialog open — its grouping and reference choices
are worth a look before an hour of integration starts. Delete that one line to
run the pipeline.

PixInsight prints nothing to the terminal in this mode, because WBPP writes to
its own console. A finished run and a failed one look alike from outside, so
read `wbpp-out/logs/*.log` for what happened; the results land in
`wbpp-out/master` and `wbpp-out/calibrated`. The scripts say so too.

This was verified against WBPP 3.0.1 in PixInsight 1.9.4: the generated
invocation classified every frame from its `IMAGETYP` header, grouped by
binning, size, filter and exposure, built master bias, dark and flat, matched
the dark and flat to the lights automatically, and calibrated them.

Rejected lights remain excluded in either layout. Repeated calibration sources
are deduplicated per destination. Name collisions receive a numbered suffix.

## Catalog upgrades

PSF Guard keeps its own tables — the calibration library among them — inside
your Target Scheduler catalog. When those tables change shape, an existing
catalog is upgraded in place the moment it is opened, and the version it sits
at is recorded beside them.

Upgrades only ever add. That keeps two machines on different PSF Guard
versions able to read the same catalog: the older build finds every column it
knows about, and simply does not see the new one. It also means an upgrade
cannot lose anything you already had.

A catalog that cannot be written — read-only storage, someone else's file —
is left as it is. PSF Guard says so in its log and opens the catalog read-only.
Calibration matching and recorded cached masters remain available when the
catalog has every column this build reads. If a stack needs a new master, it
continues without that master and explains that the catalog cannot record it.
If an older catalog is missing a required column, PSF Guard reports no
calibration library rather than failing part way through a stack. A catalog
written by a *newer* PSF Guard is read normally and can also reuse its recorded
cached masters; PSF Guard declines to change its schema or record new masters,
and says which way round the version mismatch is.

A catalog that has never held calibration data gets none of PSF Guard's own
tables written to it. They appear the first time you import calibration
frames. Starting the server can still add performance indexes — see below.

PSF Guard also adds two indexes to `acquiredimage`, the table Target Scheduler
records images in. This happens in the background once at server startup,
before catalog cache refreshes begin: building an index takes the catalog's
write lock, and a catalog that N.I.N.A. is writing to right then is left alone
and retried at the next server start. A catalog added while the server is
running likewise waits for the next start. Until then queries still work; they
just scan. Target Scheduler ships it without any, so looking up one
target's images meant reading every row of the table — seconds per query on a
catalog of a few thousand images, and repeated often enough to hold up the
Overview page. The indexes are named `idx_psf_guard_acquiredimage_target` and
`idx_psf_guard_acquiredimage_project`, so it is clear who added them and they
are safe to drop. Adding an index changes no data and no table: Target
Scheduler and N.I.N.A. keep working exactly as before, and simply run faster.

## Database transfer

**Merge catalogs** and `sync pull` copy rigs, profile bindings, and raw
calibration-frame records. They do not copy generated master rows or cached
FITS files because those paths belong to the source machine. The destination
resolves a stale raw path by basename in its configured image folders, then
builds its own master.

As with other transfer work, the UI requires a dry preview before apply.
