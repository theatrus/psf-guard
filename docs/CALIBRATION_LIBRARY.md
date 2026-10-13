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
- `psf_guard_calibration_frame_quality`
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

## Browse the library

The **Calibration** page shows one database's library: its sessions, the
frames of a session as pictures, one frame with its numbers beside its
session's, and the masters built from them. Open it with the header's
**Calibration** tab, beside Images, Sequence and Stacks. It shows the
database the header's project picker has in scope, or the first database
when none is. With a project in scope it opens on that project's rig: the
one whose frames came from the project's N.I.N.A. profile, when exactly one
did. **Rig › All** shows them all again.

**Browse calibration** in a project's Calibration report, which the
Library's calibration pill also opens, and **Open Calibration** on a
database's calibration summary in **Settings › Databases** lead to the same
page. Everything chosen on the page lives in its address,
`#/calibration?db=…`, so a reload or a shared link opens the same view: the
database, the tab, the filters, the session, the frame and how it is drawn.

### Sessions and frames

A session is the frames of one kind shot one night with one set of capture
settings: the sensor mode, frame size and binning, and then for flats the
filter and rotator angle, for darks and dark-flats the exposure and sensor
temperature, and for bias the exposure. A temperature or an angle drifts
while a set is shot, so a frame joins a session while it sits within 1.5 °C
(darks) or 1° (flats) of the next; bias is not split by temperature, since
a bias set is often shot while the sensor cools. Nights split as in
[Validity boundaries](#validity-boundaries).

The filters narrow the sessions and their frames: rig, type, filter,
exposure, a temperature range, a range of nights, the grade, whether the
frame is kept out, and frames whose file is not on disk. The session list
shows the newest night first, each session with its frames counted by grade.
Choosing a session shows its frames as a strip of small pictures, in the
order they were shot unless **Order** says otherwise, and the first one in
the viewer.

← and → move through the frames of a session; ↑ and ↓ move through the
sessions; **C** turns the comparison below on and off. The viewer keeps its
zoom and position while the frames of one session change, so flipping
through them compares the same place, and the last picture stays up until
the next one is drawn.

### Pictures

Each kind is drawn to show what can go wrong with it. A flat is divided by
its own median, per CFA site on a colour sensor, and shown as a window
around that level: **Wide** ±50%, **Normal** ±15%, **Tight** ±3%. Bias,
darks and dark-flats are shown as a window around their median of ±30, ±6
or ±2 noise sigmas, so banding shows as stripes and hot pixels as white
points. Pictures are reduced by block means, so a hot pixel stays visible
at any size. **Full size** draws the frame at its own resolution.

**Compare with session** divides a flat by the median of every flat of its
session, each first binned to at most 1,024 pixels a side and divided by its
own median. What all the flats share, dust and vignetting, divides out; what
changes between them, such as drying dew, stays. The window is then ±10%,
±3% or ±1%. It needs three flats in the session. The session's median is
worked out once, which reads every flat of the session, and kept for the
other flats. **Spots** marks the deepest spots grading measured in the
frame.

Pictures are drawn by the same queue as light previews: the page asks, the
server answers that the picture is on its way, and one poll for the whole
page waits for it. Nothing is drawn while a request waits. The pictures and
session medians are kept below the database's cache folder in
`calibration-previews`, keyed by the frame's file as it is now, the view,
size and stretch, and the drawing method, and the disk limit removes them
with the other previews. Reading a frame costs more than drawing it, so the
strip's picture and the viewer's come from one read.

### One frame

The panel beside the viewer gives the frame's file, capture time and
settings, its grade and main reason, and the numbers grading measured, each
beside the median of the other measured frames of its session: for a flat
its level, clipped share, noise and spots; for a bias or dark its level,
spread, noise, row and column banding and hot pixels. Below them are the
limits the frame crossed, each with its value and the limit.

With database management enabled, the panel can keep the frame out of
masters and exports, use a poor one anyway, or clear that choice (see
[Choosing for yourself](#choosing-for-yourself)), mark which lights it
serves (see [Validity boundaries](#validity-boundaries)), and forget it. The
session's header marks or forgets every frame of the session shown.

Forgetting a frame removes its catalog record without deleting its FITS
file. It also drops every master record that used it, including downstream
masters that used one of those masters.

### Masters

The **Masters** tab lists every master the library recorded, newest first:
its kind, filter or exposure, its source frames with their nights and
temperatures, when it was built, and the projects whose latest stacks use
it. Badges mark a bias or dark master built from fewer than five frames
(**Thin**), a flat master whose set carries changing spots, one the flat
check set aside, one from other software, and one whose file is gone.
**View** opens the master in the same inspector a stack's **Masters** button
opens (see [Inspect calibration masters](STACKING_PREVIEWS.md#inspect-calibration-masters)),
with display stretch, zoom and an unchanged FITS download, but no stack
needed. With database management enabled, **Clear generated masters**
removes them all while keeping every raw frame and catalog record. It waits
for any active stack preview; stacks build masters again when they need
them.

New frames come in through **Import** in Settings, which scans the
database's configured folders.

## Scheduler flat coverage

The NINA plugin sends Target Scheduler's `flathistory` coverage records during
grade pulls and applied reconciles. Open the database's
[Calibration page](#browse-the-library) and choose **Scheduler flats**. Filter by target,
source, filter, or status, then select the suspect coverage records and choose
**Invalidate coverage** with a reason. This requires database management access.

The next plugin grade pull or applied reconcile removes those exact, unchanged
history rows from the source scheduler database and reports the result here.
Preview-only reconcile does not apply invalidations. A full database round trip
is not required. Run this sync before the next Target Scheduler flats action:
an action already in progress has already loaded its coverage. The next action
reads fresh history and can request replacement flats, subject to its current
profile, target, cadence, and light-frame eligibility. Target Scheduler normally
considers lights from the last 45 days; older runs can require manual flats.

Coverage is not a list of flat files. Target Scheduler may record coverage after
reusing an existing flat set, and duplicate coverage can satisfy the same run.
Select every suspect record for the intended run, including duplicates and any
other sessions known to have reused the bad set. PSF Guard does not guess these
relationships from filenames or capture times. Invalidating coverage does not
reject or delete calibration files or masters, and forgetting a calibration
frame does not invalidate scheduler coverage automatically.

Each source database has a plugin-owned origin UUID. Every decision also binds
the original row ID, exact target GUID, and complete source-row fingerprint.
Newly taken flats, changed rows, and reused IDs are preserved; **Changed in
NINA** records require review. Decisions and acknowledgements remain in PSF
Guard, so stale snapshots cannot revive invalid coverage. The plugin keeps its
origin identity outside the scheduler database; moving the database or replacing
that identity starts a new source and does not apply old-source decisions.

## Masters from other software

A master dark, bias, or flat integrated by PixInsight's WBPP, Siril, or
another tool imports like any calibration frame; its `IMAGETYP` (`Master
Dark`) or `NCOMBINE` card marks it as a master, and the library shows a
**Master** badge. Such a file is used as a master as-is, never integrated
again. Two things differ from raw frames:

- **Matching is lenient.** WBPP keeps `IMAGETYP`, `EXPTIME`, `INSTRUME`,
  binning, telescope, and focal length, and drops `GAIN`, `OFFSET`, and
  `CCD-TEMP`. A master is therefore held to the rule two calibration frames
  are held to: what both sides record must agree, and a camera name or
  sensor size must still match. Exposure still has to match for a dark. The
  stack report says what the match had to take on trust.
- **Scale is settled on the way in.** WBPP's FITS masters are 32-bit floats
  normalized to 0..1 with no declared bounds. PSF Guard copies the master
  into its cache once, placing such a file on the same 16-bit scale as every
  other frame and marking a flat as already bias- and dark-subtracted. The
  original is never written to; replacing it re-adopts it.

Blank or undefined optional header values, such as `FILTER` in a PixInsight
master bias XISF, are preserved in the cached FITS master. They do not prevent
calibration, and the source file needs no header edits.

**Settings > Setups > Calibration > Masters from other software** decides
how they take part: use one whenever it matches (the default), only when raw
frames cannot build a master, or never.

## Files that move

Capture software drops calibration frames where it likes; a filer or a hand
then moves them into the calibration tree. Both ends are supported. When a
frame is imported from a new path and the library holds a row of the same
kind, file name, and capture time whose file is no longer where it was, that
row learns the new path and keeps its identity — its validity mark and the
masters built from it survive. At stack time a row whose file has moved is
followed by name, and a file is handed to the integrator once even when two
rows point at it, so a stale row can no longer fail a master with a
duplicate-input error.

## Flats that changed while they were shot

Dew or frost drying off the sensor window leaves small spots that fade from
one flat to the next. Every frame has one, so across-frame clipping keeps a
middling copy of each spot, and each light divided by that master gets a
bright bead at every spot wherever the sky is bright. So PSF Guard reads a
freshly built flat master back against its own inputs. Pixels where the
master departs from the smooth response around it by more than 10% are its
features. A frame that departs from the master by more than 5% of the local
response at a fifth of those pixels (never fewer than sixteen, never more
than one in two hundred thousand of the sensor) was not looking at the same
window. A dust shadow or a dead cluster is a feature too, but one every
frame shows the same way, and it stays. On a colour camera the smooth
response is judged per CFA phase.

- When the frames that disagree are a minority, the master rebuilds from the
  rest and the stack card says how many were left out.
- When the set as a whole disagrees with itself, the next-nearest flat set
  takes its place and the card shows both.
- When there is no other set, the master serves and the card says the set
  was unstable and needs retaking.

The check runs once per build, before the master is published, so nothing
else can pick up a master that is about to be set aside. The master's record
keeps the judgement: a cached master repeats its note, and a set that was
set aside is remembered rather than read again. Flat masters built before
this check rebuild once; bias and dark masters stay as they are.

## Frame grades

PSF Guard grades every raw bias, dark, dark-flat and flat in the library
against the frames that should agree with it. Seiza measures and judges;
PSF Guard groups the frames and keeps the results in
`psf_guard_calibration_frame_quality`. Each frame gets one of three grades:

- **OK**: nothing crossed a limit.
- **Suspect**: something crossed the first limit. The frame stays in use.
- **Poor**: something crossed the second limit. Masters, WBPP exports and
  calibration reports leave the frame out.

A master from other software is not graded.

### What is judged

A frame is judged against its peers:

- **Bias, darks and dark-flats**: every frame that could stand in for it,
  from any night: the same kind, rig and sensor settings (gain, offset,
  readout mode, binning), with a matching exposure and temperature.
- **Flats**: the other flats of its session: the same night, rig, filter,
  rotation, binning and frame size.

| Kind | Check | Suspect | Poor |
|---|---|---|---|
| Bias | level, distance from the set's in noise | 1 | 3 |
| Bias, dark | noise, ratio to its peers | 1.1 | 1.3 |
| Bias, dark | row or column banding, ratio to its noise | 0.25 | 0.5 |
| Dark | level above the quietest peer, in that frame's spread | 0.5 | 1.0 |
| Dark | hot pixels, ratio to its peers | 1.5 | 3 |
| Flat | samples clipped | 0.05% | 0.5% |
| Flat | brightest channel, share of full scale | 80% | 92% |
| Flat | dimmest channel, share of full scale (lower is worse) | 10% | 3% |
| Flat | light pattern off the session's | 2% | 5% |
| Flat | clear spots in one frame | 100 | 500 |
| Flat | spots that change across the session | 3 | 20 |

Darks are compared for noise and hot pixels only with the peers whose
level passed, since light raises both. Most checks need three or more
peers; a frame with fewer is judged only on what it shows alone, such as
clipping or banding. A flat's level is judged against a 16-bit full scale.
A flat's temperature is never judged: a flat shot warm with the cooler off
can be a good flat.

A spot is a small patch that passes less light than the response around
it. Dust makes spots that sit in every flat of a session at the same depth;
those are left alone. Dew or frost drying off the window makes spots that
fade from one flat to the next; a flat holding many of those is poor.

### When frames are graded

A background job grades the library: when the server starts, after each
import, after a frame is forgotten, and two minutes after remote uploads of
calibration frames stop arriving. It reads each frame once, on the workers
the background budget allows, and waits while interactive work runs. Later
runs read only new frames, frames whose file changed, and frames an older
version of the checks measured; grading again needs no pixels.
`GET /api/db/{db}/calibrations/quality` reports its progress and how many
frames have each grade.

A file the job cannot read, as on an unmounted share, waits for the next
run. A frame Seiza cannot measure, such as a flat smaller than 32 pixels a
side, is recorded and left ungraded. An ungraded frame stays in use.

### Choosing for yourself

The calibration page shows each graded frame's grade and main reason, such
as **Poor · stray light** or **Suspect · noise**, with the numbers behind
it, and each session counts its frames by grade and those kept out. With
database management enabled, each frame offers:

- **Keep out**: leave the frame out of masters and exports whatever its
  grade. Unlike **Forget**, the frame stays in the library, so a rescan does
  not bring it back.
- **Use anyway**: put a frame the library keeps out back in use.
- **Clear**: let the grade decide again.

The choice survives regrading, rescans and database copies.

### What leaving a frame out does

A frame left out changes the set a master is built from, so the master and
every stack that used it are built again. The stack card lists the frames
left out and why, and so does a light's calibration explanation. A WBPP
export leaves the frame behind, and its dialog counts it.

Grades never stop calibration. If they leave a light with no flat, or with
neither a bias nor a dark, the light is one that
[cannot be calibrated](#lights-that-cannot-be-calibrated): the report lists
it, and a stack goes on without that master, as for any other gap.

### Darks that caught stray light

A dark shot while light reaches the sensor, such as dawn through an open roof
or a cap left off, carries the right exposure, gain, offset and temperature, so
header matching accepts it. Only the pixels give it away: the whole frame sits
above the bias, usually more with each frame as the sky brightens, and the
extra light follows the optics' vignetting. On one rig a 300 s set read 504,
504, 505, 588, 774 and 1,172 ADU against a 503 ADU bias. Across-frame clipping
drops the worst frame but keeps the milder ones, and subtracting that master
pushes every light's sky below zero and carves a bowl that later shows as
colored corners.

The dark check catches this. A dark more than one noise sigma above the
quietest frame that could stand in for it is poor, with the reason **stray
light**. Peers come from every night, so a morning whose darks all leaked
is still caught by a clean set from another night. A lone frame with
nothing to compare against stays in use. The frame panel gives each measured
dark's and dark-flat's level and spread in ADU beside its session's. The
verdict also goes into the frame's `stray_light`
column, which older PSF Guard builds read. Master building applies the same
rule to its own inputs, which catches a dark the job has not reached yet.

## Upgrades and backups

PSF Guard keeps its own tables inside the scheduler catalog and upgrades them
in place when a newer build opens the catalog. Before any upgrade that changes
the file, it takes a consistent copy beside it with SQLite's `VACUUM INTO`:

```
scheduler.sqlite.psf-guard-backup-v4-20260902-031500.sqlite
```

The name carries the schema the catalog had and the UTC time. The three newest
backups of a catalog are kept; older ones are removed as new ones arrive. If
the copy cannot be written (disk full, folder read-only), the upgrade does not
run, the catalog stays readable at its old schema, and the log gives the path
to clear. An upgrade never reads frame files; anything that needs them, such
as recording readout-mode names, runs after the server is up.

## Safe matching

PSF Guard only uses candidates that agree with every known hard setting:

| Master | Required match |
|---|---|
| Bias | camera, dimensions, channels, binning, gain, offset, readout mode, Bayer layout |
| Dark | bias fields, exposure within 0.05 seconds, temperature within 3 C, shot within the dark reach of the light |
| Dark-flat | flat sensor settings, exposure, and temperature |
| Flat | bias fields, filter, telescope, and focal length within 1 mm |

If the light records a hard setting and a candidate omits it, that candidate
does not match. If the light itself omits a setting, PSF Guard cannot use that
field as a gate. The readout mode counts whichever way the camera driver
spelled it: N.I.N.A. writes a name such as `Extend Fullwell 2CMS` rather than
a number, and PSF Guard keeps that name and compares it case-insensitively,
so a High Gain dark never serves an Extend Fullwell light. The name is the
one setting where a candidate that recorded nothing is still accepted: it is
a label the capture software chose, and frames from software that writes a
number or nothing would otherwise stop matching lights that record a mode.
Two frames that both record a mode must agree, and a master never mixes two
names. For a library built before names were kept, the server reads them off
the frames' headers after it has started, in the background and in chunks,
logging its progress; a frame whose file is unreachable is tried again at the
next start and matches as it did before until then. A match still needs a positive camera-name or sensor-size
identity; wholly unknown sensors never match. It sorts safe candidates by
distance from the light's capture time and uses at most 64 frames per master.

The frames feeding one master must also cohere with each other, which the
build enforces after selection so per-light selections stay identical across
a stack group: candidates cluster by sensor temperature (within 1 °C, the
same gate the stacker itself enforces), and flats also cluster into one
session (within a day of each other — dust moves between sessions). The
nearest cluster with enough frames builds the master, so a stray single flat
near the lights cannot orphan a complete session from a week earlier.

Darks keep for months, so a dark master can pool several nights, nearest
first. Three settings under **Settings › Calibration › Dark masters** shape
it:

- **Dark reach** (default 183 days): a dark shot further than this from the
  light does not match. A dark marked to serve only lights before or after it
  follows its mark instead, however old.
- **Complete night** (default 10 darks): when the nearest night alone has
  this many matching darks, the master uses that night only. With fewer, the
  nights within reach pool.
- **Dark choice** (default *Complete set first*): the nearest temperature
  cluster with a complete night's worth of darks builds the master, and a
  nearer, shorter cluster builds it only when no cluster is complete. Darks
  match a light within 3 °C but cluster within 1 °C, so two warm darks shot
  the night after the lights could otherwise beat eighty at the set point a
  week later; a two-frame master puts its own read noise into every light,
  and the stack shows it as row banding. *Nearest set* keeps the older rule:
  the nearest cluster of two or more, however short. Bias masters follow the
  same choice, with the same count for a complete set. A WBPP export places
  the same frames, so WBPP builds the same masters.

### Validity boundaries

Nearest-in-time is the right default, but it cannot know about a dust
cleaning, a re-spaced imaging train, or any other change that makes older
calibration wrong for newer lights. When nights lack their own frames, mark
the boundary yourself on the [Calibration page](#browse-the-library): a
session's header marks every frame of the session, and the frame panel one
frame. A night splits where the catalog's lights are never captured, as in
[Sky coverage](SKY_COVERAGE.md), so dusk and dawn flats share the date of
the night between them, as N.I.N.A.'s `DATEMINUS12` folders do. The coverage
report and export folders use the same nights. A catalog with no lights
splits at noon UTC. The session header can also **Forget session**,
removing its records (and dependent masters) in one step without touching
the FITS files. Mark frames usable **After only** (a set shot right after
the change) or **Before only** (the final set shot before it). Marked frames
carry a badge, and matching never selects them across their boundary, in
either direction, for any kind. **Both** clears the boundary. A light captured exactly at the boundary
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

The defect pass removes pixel-scale impulses, not star images. For sky flats,
let the sky drift or dither between exposures. Flat integration subtracts the
available bias/dark calibration, normalizes each exposure to its median
brightness, then clips each pixel's samples around their temporal median.
Its sigma estimate comes from the median absolute deviation (MAD), which is
less sensitive to star-contaminated samples than a mean/variance estimate.
The retained samples are averaged, preserving the fixed dust shadows and
vignette that the flat is meant to measure.

Without star masking, rejection needs at least three flats; two are only
averaged. Use enough well-separated sky flats that most samples at each pixel
are free of stars.
A star that stays on the same pixels, or contaminates half or more of the
samples there, cannot reliably be distinguished from the sensor response.
Keeping that contaminated majority can even strengthen its imprint compared
with an ordinary average. A clean majority is necessary, not a guarantee of
complete removal: with few inputs, noise and faint star wings can still leave
residuals below the clipping threshold. Move stars clear of their full footprint
between enough exposures and inspect the resulting master, especially around
saturated stars.
The spatial defect pass cannot remove a broad star image that survives this
combine. When no dark master
exists anywhere in a stack's plan, the stack instead runs the same impulse
filter over each calibrated light, and the card says so.

### Native sky-flat masks

Enable **Mask stars in flats** under
**Settings > Setups > Calibration > Flat masters** to exclude detected stars
before flat integration. The setting is off by default, applies to every
database on this server, and persists through a restart. It needs no
StarXTerminator or other external tool. Imported masters
are used unchanged; this option only affects masters built from raw flats.

The native detector analyzes each input, expands stellar footprints to include
halos, and adds masks around multi-pixel clipped cores when the raw encoding
or headers establish a clipping ceiling. The original sensor samples are not
resampled, smoothed, or filled. Integration excludes the masks, then applies
median/MAD clipping to the remaining samples. Two remaining samples are
averaged without clipping. This can recover a pixel from a minority of usable
exposures rather than accepting the star-contaminated majority.

Every output sample must retain at least two input measurements. Otherwise
the flat build fails with coverage counts and is not published or applied.
The stack can continue without that flat, with the reason shown in its
calibration warning. Successful masters record masked-sample counts and
retained coverage in their FITS headers, catalog statistics, and build logs.
Coverage of only two retained exposures also appears in the stack warning,
including when the master is reused from cache.

Coverage means unmasked retained measurements, not certified artifact-free
pixels. Faint halos can escape detection, and the mask does not reconstruct
response hidden by a star in every exposure. Isolated detector impulses stay
under the existing defect-suppression policy instead of growing star-sized
masks; the statistics distinguish unmasked clipped impulses and inputs whose
clipping ceiling is unknown. Inspect the master and capture enough separated
flats to measure the whole sensor response.

Masked and unmasked masters have separate cache identities. Changing the
setting affects new stack requests; a queued or running stack keeps the value
captured with its cache key.

A master that fails to build does not fail the stack. The frames integrate
without that master, and the stack card's calibration warning says which
master was skipped and why. A master whose input master failed —
a flat whose bias did not build — is skipped rather than built without it,
because a flat normalized with the bias pedestal still in it would miscorrect
every light. Resume checkpoints and source-frame searches compare the masters
actually applied, not just the selection, so a build that fails or recovers
between runs forces a clean rebuild instead of mixing calibrations.

## Project coverage report

Each project card on the Library has a **Calibration** action that reports
how the library covers that project's lights (its **Browse calibration**
button opens the [Calibration page](#browse-the-library) for the project's
database): per kind, how many frames
match and which capture sessions they span; and per imaging night and
filter, the flat session a master would build from, its distance from the
lights, and whether the night has its own flats. The dark column gives what
one dark master would use: its frames, the nights they span and the nearest
night's distance; hover for every matching dark within reach. A night with
no flat shows the nearest flat refused only for its rotator angle, how far
off it is and the limit, which is the usual cause when a rotator moved.
Masters from other software for the lights' camera, sensor size and (for
flats) filter are listed under the night: used, matching but not used under
the external-master setting, or not used with the readings that disagree,
such as `rotation light=92.6deg master=94.7deg`. One representative light
per night and filter is matched exactly as a stack build would match it, so
the report describes what WOULD apply, not just what files exist. Warnings
call out kinds with no matches, nights without same-night flats, and flats
more than a month from their lights.

The report leaves rejected lights out, since no stack uses them, so a night
you rejected for cloud does not show up as lacking flats. When the project
has rejected lights, **Include N rejected lights** adds them back.

### Lights that cannot be calibrated

A light cannot be calibrated properly when:

- no flat matches it, though the library holds flats for its sensor (for
  example, every flat for its filter was shot at another rotator angle);
- neither a bias nor a dark matches it, though the library holds either; or
- the flats that match were shot further from it than the **Flat age limit**
  (**Settings › Calibration › Flat masters**, default 60 days; 0 lifts it).

Each rule applies only where the library could have filled the gap, so a rig
that never shoots flats, or never shoots darks, is not affected. The report
lists each night whose lights cannot be calibrated. **Check lights**, at the
foot of the report, reads every light of the project that is not already
rejected and lists those that cannot be calibrated, by night, filter and
reason. **Reject N lights** then sets them Rejected with the reason, so
Target Scheduler shoots them again. It refuses, and asks for a new check,
when the list changed after it was shown.

Stacks keep such lights unless **Settings › Calibration › Stacks › Leave out
lights that can't be calibrated** is on. With it on, and calibration on or
automatic, stack previews leave them out, so they never mix with properly
calibrated frames: the stack card counts them under **can't calibrate** and
each frame's decision gives the reason.

### Calibration health

PSF Guard also judges how well the library calibrates each project, so the
Library can flag the projects worth a look without anyone opening a report.
Every light that is not rejected is read and matched as a stack would match
it, and what it would get is judged on two levels.

**Poor**: the light cannot be calibrated properly.

- Nothing in the library matches its camera and settings, though the
  library holds frames for other cameras. A camera that has never had
  calibration frames shows here; the rules above say nothing about it.
- One of the rules above: no flat, or no bias or dark, or flats past the age
  limit.
- Its dark master would use fewer than five frames, or its bias master
  would, when no dark serves it. A short set puts its own noise into every
  light.
- Its flats carry changing spots into their master: spots that come and go
  across the set, such as drying dew, sit in most of its frames.

**Check**: worth a look; the light still calibrates.

- Its flats were shot more than 30 days from it.
- Its flat master takes flats from more than one night.
- Its masters use frames graded suspect.
- Frames kept out by hand leave its bias, darks or flats under five frames.
- Its bias master would use fewer than five frames while a dark serves it,
  or a few changing spots sit in most of its flats.

A light takes its worst finding, and a project its worst light. The Library
shows a **🧪 Poor** or **🧪 Check** pill after the grading and date pills,
on the compact row and the card; it opens the Calibration report, whose
**Quality** section lists each finding with its lights by night and filter.
**Show › Calibration issues** narrows the Library to those projects.

The image views mark the same lights. In Images, Sequence and a mosaic's
sequence, a **🧪** chip under the score chips is red on a Poor light and
yellow on one to check; hover it for its findings. The chip has its own
toggle beside the 🌙 and layers toggles, and in **Settings › Review**.
Images has a **Calibration** filter: **Poor**, **Check**, or both. Sequence
adds **Poor calibration** to **Select**, so the lights can go straight to
**Review rejection**. In the Calibration report, each Quality finding, each
night that can't be calibrated, and each line of **Check lights** opens
those lights in Images, filtered to that finding and its filters.

Reading every light takes time, so this runs in the background, never on a
request: after each grading pass (which follows the server start, imports,
syncs, forgets and remote uploads), when a light's grade changes, and when a
frame is kept out or used anyway, a validity mark changes, or the
calibration settings change. It uses the background worker share and waits
while interactive work runs. The results are kept below the database's cache
folder, each filed under a key of the library and its grades, the project's
lights, the calibration settings and the rules, so a project is read again
only when one of those changed. Accepting a light, or setting it back to
pending, changes no key; rejecting one does. Until a project is read again,
a result that no longer holds shows no pill. The image views keep showing its
marks, so rejecting lights does not clear the rest until the pass ends.

## Stack previews

Stack previews build masters on demand with `seiza-stacking`. Each master needs
at least two inputs. Bias and dark masters use a two-pass, leave-one-out
sigma-clipped mean. Flat masters use median/MAD clipping after per-frame
calibration and normalization. Both use 3-sigma low and high thresholds;
two-frame sets skip rejection. The master FITS records the method in `REJMETH`,
the clipping thresholds, source count, and rejected-sample counts. Build logs
and catalog statistics also record the method and counts.

Flat integration decodes each source once and uses temporary disk storage for
the normalized samples, then combines bounded tiles. This keeps the tile
memory bounded as the number of flats grows. Scratch files live in the
database's calibration-master cache directory and are removed when the build
finishes or fails. Allow roughly 16 GB of free space for 64 mono 61-megapixel
flats, or three times that for already-RGB inputs. Raw source files are never
modified. If scratch storage fails, the stack reports the failed master and
continues without that calibration; it does not silently disable clipping.

Generated masters live below:

```text
<cache>/<database>/calibration-masters/
```

The master filename is content-addressed by the source-frame UUIDs, current
file fingerprints, input master UUIDs, algorithm version, flat star-masking
mode, and master-cache version. A changed bias therefore invalidates dark and
flat masters that used it; a changed dark-flat invalidates its flat master.
A later stack reuses a valid file. The database records its source set, input masters, and Seiza
version.

Since a new build version, a new upstream master or another masking mode
writes a new file from the same frames, old masters stay behind. Once an
hour, while no stack build runs, PSF Guard removes from the master folder:

- half-written files a stopped build left (`*.fits.tmp-*`), a day old;
- masters a newer master built from exactly the same frames replaced, once
  unused for a day and absent from every stack index and job manifest. Masters
  from other frames, such as another night's flats or another gain's darks,
  never count as replacements.

When the master folder's volume is over its disk limit and removing masters
can bring it back under, masters unused for a week may go too, least recently
used first (see [Disk use](STACKING_PREVIEWS.md#disk-use)). Never removed: a
master the catalog does not record or whose source frames are no longer on
disk, since neither could be built again; anything while the catalog is
read-only or was written by a newer build; anything while a stack index or
manifest cannot be read; and a master's catalog record. A removed master that
a stack needs again is built again from its frames; until then the stack's
**Masters** view reports it missing.

The stack card reports `Calibration applied`, `Calibration set incomplete`, or
no calibration. It also shows input counts and any missing-file warning.
Calibration happens on raw CFA samples before debayering.

A master's catalog record also keeps what Seiza reported about each input
(samples kept, clipped and masked) and how many defective pixels it
replaced. A bias or dark master built from fewer than five frames is marked
thin, since a short set puts its own read noise into every light. A flat
master records Seiza's verdict on its set of flats: spots that change
across the set but sit in most of its frames survive into the master.

Lights that need different master sets are partitioned into calibration
sessions, and each light uses its session's masters. The **Masters** tab of
the [Calibration page](#browse-the-library) lists every recorded master. Choose
**Masters** on a completed mono or color stack to inspect its recorded files, their session and
channel usage, and available rejection statistics. The inspector supports
display stretch, native-size zoom, and unchanged FITS downloads. Missing cached
masters are reported without rebuilding them. See
[Inspect calibration masters](STACKING_PREVIEWS.md#inspect-calibration-masters).

## Export

Project and target exports add the raw frames matched to each selected light.
Two layouts are available; the default is unchanged.

**Grouped by target**, the default:

```text
BIAS/
DARK/<exposure>_G<gain>/
DARKFLAT/<exposure>_G<gain>/
<target>/FLAT/<filter>/SESSION_<night>/
<target>/LIGHT/<filter>/
```

**WBPP**, one root per frame type, for WeightedBatchPreprocessing:

```text
bias/G<gain>/
darks/<exposure>s_G<gain>/[DARKSET_<night>/]
flats/<target>/<filter>/SESSION_<night>/
lights/<target>/<filter>/SESSION_<night>/[DARKSET_<night>/]
```

Dark flats land in `darks/` beside the lights' darks. WBPP has no dark-flat
type: a dark flat is a dark it pairs to a flat by exposure, so a separate
folder would only mean adding one to WBPP twice.

Each light takes the calibration frames a stack would build its masters from:
the coherent set nearest the light per kind, not everything the library
matched. Flats stay under their target because PSF Guard matched them to that
target's lights, and under the night they were shot because two nights of one
target can need different flats for one filter. Merging either would have WBPP
integrate both into a single master flat.

In the WBPP layout the light's path carries the same night as its flats. The
runner passes `keywords=SESSION` with keyword grouping on, and WBPP reads the
value out of the `SESSION_<night>` folder: each night's lights calibrate with
that night's flats, bias and darks carry no session and serve every night, and
the nights integrate together afterwards. A light with no flats has no session
folder. The grouped-by-target layout only sorts the flats by night.

Darks get the same treatment when lights in one export need different ones:
when lights of one exposure and gain were matched to different dark masters
(say, each to its own complete night), each set of darks goes in a
`DARKSET_<night>` folder for its nearest dark's night, with a short digest when
two sets share a night, and each light's path carries its set. The runner then
passes `keywords=SESSION;DARKSET`. WBPP lets a calibration group serve a light
only when every keyword the group carries equals the light's, and prefers the
group matching the most (`keywordsMatchCount` in WBPP's
`BPP-FrameGroup.js`), so tagged darks serve only their own lights while bias,
dark flats and flats, which carry no `DARKSET`, serve every light. When the
lights of an exposure share one set of darks, nothing changes. This rule was
read from WBPP's source; it has not yet been run through PixInsight.

Choose the layout with `--layout wbpp` on the CLI, the `layout` query parameter
on the export download, or in the dialog every Export action on the overview
opens. The dialog starts from the **Export** default in Settings → Setups,
which is server-wide and shared by the desktop and browser apps. With the WBPP
layout the dialog also asks for WBPP's settings (quality preset, Fast
Integration, drizzle, autocrop, light rejection), which the runner script
passes; see [Stacking with PixInsight's WBPP](WBPP.md), which also covers
running WBPP on the server from the Library.

The dialog also asks whether to include ungraded lights (on by default, as
the stack previews do; rejects are never exported) and how the files land:

- **Copy** stands on its own: a clone or hard link where the filesystem
  allows, a copy elsewhere.
- **Link to the originals** writes symbolic links, so the tree costs no space
  and can point at a network mount. Whatever reads it must see the originals
  at the same paths, which suits a PixInsight on the same machine or on one
  that mounts the same share at the same place.
- **Reference in place** copies nothing. The export is three scripts:
  `run-wbpp.js` carries the frame list and hands it to WBPP from inside
  PixInsight, and `run-wbpp.sh` and `run-wbpp.cmd` launch it with the output
  folder. No frame travels on a command line, so the list has no size limit.
  The script includes WBPP from its standard install path per platform; edit
  the `#include` line for an install elsewhere. It can also run from
  PixInsight's Script > Execute Script File, writing to a `wbpp-out` folder
  in your home directory. The dialog's **Server path** is the folder the
  list is relative to, prefilled with what the database's image directories
  share; **PixInsight path** is the same folder on the PixInsight machine,
  if it differs. Server path `/mnt/barium/astrobin` and PixInsight path
  `P:\` list the frames as `P:/_ByTelescope/...`; `psfSourceRoot` at the top
  of the script is that value and can be edited later. A frame outside the
  server path keeps its full path. The originals' paths carry no session
  folder, so the script tells WBPP each frame's session itself and each
  night's lights still take their own flats. This mode needs the WBPP
  layout. On the CLI it is
  `--placement reference` with `--local-root` and `--remote-root`;
  `--placement symlink` links. In the browser, where an export is a zip
  download unless the database has a server export directory, **Scripts
  only** downloads the same three scripts as a small zip, pointing at the frames
  where they are on the server's storage; `placement=reference` on
  the download URL does the same.

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

The reference runner was also verified there: WBPP reads its parameters
from `Runtime.jsArguments`, which the core makes read-only, so the script
includes WBPP's entry file inside a function where a Proxy named `Runtime`
answers `jsArguments` with the list built in the script and everything else
from the real object, then calls WBPP's entry point as `WBPP.js` does. WBPP
reads a grouping keyword's value out of a frame's path, so the script also
replaces that reader with one that answers `SESSION` and `DARKSET` for its
own frames;
WBPP consults it on every regroup, so the value holds. It also checks that
every frame exists before handing the list over, because WBPP drops a frame
it cannot find without a word, and a wrong root would otherwise open an
empty dialog.
`#engine v8` at the top is what `WBPP.js` declares; without it the core
compiles the include with an engine that rejects WBPP's classes.

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

Frame grades live in a table of their own,
`psf_guard_calibration_frame_quality`, added without raising the schema
version, because a build refuses to import into, sync into or record
masters in a catalog whose version is newer than its own. PSF Guard 0.11
therefore keeps working on a catalog this version has graded. It ignores
the table: it leaves out the darks the grade marks for stray light, which
it reads from the frame's own stray-light mark, and uses frames graded poor
for other reasons, or kept out by hand, as before.

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
Library page. The index names, `idx_psf_guard_acquiredimage_target` and
`idx_psf_guard_acquiredimage_project`, show who added them and that they
are safe to drop. Adding an index changes no data and no table: Target
Scheduler and N.I.N.A. keep working exactly as before, and simply run faster.

## Database transfer

**Merge catalogs** and `sync pull` copy rigs, profile bindings, raw
calibration-frame records, and their grades. A choice to keep a frame out or
use it anyway travels too; when both catalogs hold one, the later choice
wins. They do not copy generated master rows or cached FITS files because
those paths belong to the source machine. The destination
resolves a stale raw path by basename in its configured image folders, then
builds its own master.

As with other transfer work, the UI requires a dry preview before apply.
