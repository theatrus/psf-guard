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
- `psf_guard_flat_response`, `psf_guard_flat_comparison` and
  `psf_guard_flat_light_evidence` (see [Flat groups](#flat-groups))

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
**Calibration** tab, between Sequence and Stacks. It shows the
database the header's project picker has in scope, or the first database
when none is. On Calibration the header's **Rig · target** control names
the rig alone: the page shows a rig's library, never one target's. For a
project shot by several rigs it offers only those rigs, and choosing one
opens the same project there; the target stays in the address, so Images
shows it again.

### Project scope

Opened from a project's view (Images, Sequence, Stacks or a project's
Calibration report), the page shows the calibration that project uses, with
an **M 44 on Askar 120 ✕** chip first in the filter bar and the heading
**Calibration · for M 44 · Askar 120**. Opened from the Library, the Sky or
with All projects, it shows the whole library. A light's calibration panel
accounts for frames it matched, was refused or lacks, most of which the
scope leaves out, so its links open the rig's whole library narrowed to
that kind and night.

The scope is what the project's lights use: each light is matched the way a
stack matches it, then reduced the way a stack builds its masters, to the
one coherent set of frames each master takes (for darks, the nearest
complete session) or the master from other software the policy adopts.
Matching alone has no age limit for flats, so an old session at the light's
filter matches without being used; it is not in the scope.

- **Frames**: the sessions holding a frame some light's masters use: its
  rig, filter, exposure and settings, its nights, the frames kept out, and
  flat grouping's choices (flats from another angle, corrected flats, the
  side of a change). Corrected flats bring the two sessions the change's
  map is made from. Flats a light keeps only so its stacks keep their keys,
  from the far side of a change, are not in it.
- **Masters**: the recorded masters whose every source frame sits in the
  set one light would build that master from, and those the project's
  latest stacks used.
- **Flat groups** and their changes: only the families the scope's flat
  sessions belong to.

A project whose lights span several calibration rigs, after a camera swap
or at another binning, shows them all, grouped by rig, and the chip says
how many. With one rig the Rig filter opens on it; with several, on All.

**✕** on the chip, or **Show all of Askar 120's calibration** under the
heading, widens to the whole library and keeps the Rig filter, so you see
everything for that rig; the Rig filter's **All** then shows the whole
catalog. **Clear**
removes the scope with every filter. Choosing another project in the
header moves a scope to it; a page you widened stays wide. The address keeps
the scope as `calProject=<id>`. **Scheduler flats** and **Duplicates** read
the whole library.

The server works the scope out on a connection of its own, from the light
headers the calibration health pass keeps. Lights the pass has not reached
are read by a background job sized like other interactive work; the page
says how many are still being read, the scope grows as they arrive, and
the job stops once no view has asked for a minute. The scope is kept until
something it depends on changes: a frame, its validity mark or whether it
is kept out, flat grouping's measurements, the recorded masters, the
settings, or the project's own lights. Grading the library in the
background changes none of these unless it keeps a frame out. After a
change the last scope is served while one refresh works out the next.

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
[Validity boundaries](#validity-boundaries). A session's address carries
its middle temperature or angle rounded to a whole degree, so a link to it
keeps working when a colder or warmer frame joins it, unless that frame moves
the middle across a half degree.

The filters narrow the sessions and their frames: rig, type, filter,
exposure, a temperature range, a range of nights, the grade, whether the
frame is kept out, and frames whose file is not on disk. **Duplicates**
lists exposures recorded more than once instead; see
[Duplicates and their repair](#duplicates-and-their-repair). A frame the catalog
records outside the database's image and calibration folders, as a catalog
synced from another machine may, counts as not on disk: the server never
reads it. The session list
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

With an editor account, the panel can keep the frame out of
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
needed. With an editor account, **Clear generated masters**
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
**Invalidate coverage** with a reason. This takes an editor account.

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

**Settings › Calibration › Matching › Masters from other software** decides
how they take part: use one whenever it matches (the default), only when raw
frames cannot build a master, or never. A rig can choose for itself.

## Files that move

Capture software drops calibration frames where it likes; a filer or a hand
then moves them into the calibration tree, often by copying them and
deleting the originals later. Both ends are supported, and so is the window
in between, when one frame is on disk twice.

Two files hold the same exposure when they agree on:

- the kind;
- the capture time to the second, from the header's `DATE-OBS`, or
  `MJD-OBS` when it has no date text;
- the file name, case aside;
- the rig;
- the capture settings the header records: exposure, gain, offset,
  temperature (within the dark temperature tolerance), filter, binning and
  frame size;
- the file size.

When both files are on disk, their first and last 32 KiB must also be byte
for byte the same. That covers the header and the ends of the pixels, where
two different exposures never share sensor noise, so a different frame that
happens to share a name and second is never taken for a copy.

When one file is gone there is nothing left to compare, and the library's
record decides, but only for a frame whose header gives a capture time. A
row for an undated frame, as many masters are, is never matched to another
file on its record alone. No folder name plays a part.

When an import meets a file that holds an exposure the library already
records at another path:

- If the recorded file is still on disk, the new file is remembered as a
  second copy and no row is added. Later scans skip that copy without
  reading its header while the recorded file is there.
- Once the recorded file is gone, the row learns the new path and keeps its
  identity: its uuid, grade, keep-out or use choice, validity mark, and the
  masters built from it. It takes the new file's header with it.
- A remembered copy whose own file is gone is forgotten at the next import.

At stack time a row whose file has moved is followed by name, and a file is
handed to the integrator once even when two rows point at it.

### Duplicates and their repair

A library imported by an older build can record one exposure more than
once: a row for the drop-folder copy that the filer has since deleted, which
shows as **Missing**, or two rows for two copies that are both still on disk.
A master built while both rows were there may have averaged one exposure
twice.

Tick **Duplicates** among the Calibration page's filters
(`calDupes=true` in the address) to list every exposure recorded more than
once. Each group shows its files, which are missing, and which row a repair
keeps. **Rig** and **Type** narrow the list, and the view offers no other
filters. A row whose file is gone only joins a twin that is on disk. A group
whose copies are all on a disk that is not mounted is left alone. When only
some of its copies are, a row on the unmounted disk reads as missing and
merges into a copy on disk; the kept row moves to that copy, and a later
scan moves it back if that copy goes.

**Repair duplicates…** previews the repair: how many rows merge, how many of
them had lost their file, how many were second copies on disk, and which
masters are built again. The repair covers every rig and type in the
library, whatever the filters show. **Apply** then, for each exposure:

- keeps the oldest row, and moves it to a twin's file when its own is gone,
  taking that file's header and fingerprint;
- carries over a grade, the newest keep-out or use choice (a cleared choice
  counts when it is the newest), a validity mark, and a remote upload's
  record, then deletes the other rows;
- remembers second copies still on disk, so scans skip them;
- records each merge, so a sync honours it (below);
- removes every master built from a deleted row or from a kept row that
  moves, and every master built on one of those, so stacks build them again.
  Stacks key on the frames they use, so their previews rebuild too.

A master that was being built while the repair ran is not recorded if a row
it used has gone; the next stack builds it again.

No file is moved or deleted. Apply is refused when the library, or which
files are on disk, has changed since the preview; preview again. A second
repair finds nothing to do. Repair needs database management: the desktop
app, or a server started with `--allow-database-management`.

From the command line, `psf-guard calibration repair-duplicates <db>
--dry-run` prints the same plan without writing, and without `--dry-run`
applies it. A server open on the same catalog sees the change at its next
scan, regrade or restart. For a dry run against a catalog copied from another machine,
`--map-prefix /mnt/share=/Volumes/share` reads the recorded paths through
this machine's mount. The API is `GET /api/db/{db}/calibrations/duplicates`,
which returns the groups, the plan and a `token`, and
`POST /api/db/{db}/calibrations/duplicates/repair` with that `token`, which
answers `409` when the plan has changed.

#### Repairs and sync

A pull copies the repair's merge records with the calibration library, by
frame uuid:

- A row a repair here merged is never inserted again from a catalog that
  still holds it. The row it went into keeps the file the repair chose, and
  a later choice made on the merged row at the source arrives on the kept
  row.
- A row the source's repair merged, which this catalog still holds, is
  merged here too, and the masters built from it go.

Only a pull carries the calibration library. The catalog a pull reads from
keeps its own duplicates until it is repaired there. The pull's preview
counts the merged rows it leaves out and the rows it merges.

## Flats that changed while they were shot

Dew or frost drying off the sensor window leaves small spots that fade from
one flat to the next. Every frame has one, so across-frame clipping keeps a
middling copy of each spot, and each light divided by that master gets a
bright bead at every spot wherever the sky is bright. So PSF Guard reads a
freshly built flat master back against its own inputs. Pixels where the
master falls below the smooth response around it by more than 10% are its
features. Only dips count: dew, frost and dust take light away, while a
star in a sky flat adds it and sits on other pixels in each frame, so a
set of tracked sky flats is not mistaken for a drying window. A frame that departs from the master by more than 5% of the local
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
this check rebuild once; bias and dark masters stay as they are. A
judgement made before stars stopped counting is made again the next time
its master is used.

## Flat groups

Dust moves, arrives and leaves between nights. A light divided by flats
from the wrong side of such a change carries the difference as a faint
ghost. So PSF Guard measures how alike each night's flats are, keeps
nights that differ out of one master, and gives every light flats from its
own side of each change.

### Measuring

After each grading pass, a background stage reads every flat session once
more: one night of one rig, filter, focal length, sensor mode and rotator
angle, with at least two flats in use. Two flats make a response as two
make a master; their combination is a mean, noisier than a median of
three or more, and the comparison allows for that. Focal lengths within the rig's
tolerance (1 mm by default) count as one; flats shot through a reducer
are never compared with flats shot without it, and flats that record no
focal length join the rig's only one, or keep to themselves when it has
several. It takes at most 16 flats of a session, bins each to at most
1,024 px on the long side, divides each by its median and keeps the
sigma-clipped median of them: the session's **response**. The measuring
itself is Seiza's (`seiza_calibration::flat_response`). Stars that move
between frames drop out. Stars that sit still, as in tracked sky flats,
are masked when two responses are compared.

Two responses are compared as later ÷ earlier, after the dark-flat or bias
level is taken out. Two numbers come from the ratio, each in percent of
the sky background a light would be left with if it took the other night's
flats:

- **Dust**: the deepest shadow that appeared, left or moved. A spot counts
  only where one of the two responses holds a dip; a bright leftover star
  is not dust.
- **Light pattern**: the largest broad difference, such as a changed
  vignette, after a straight gradient is taken out. Background extraction
  removes a gradient from the stack.

Sessions that could share a master, and each session and the next, are
compared. The numbers are kept in the catalog without limits applied, so a
changed limit regroups at once and reads no pixel.

Measuring is a job of its own, **Measuring flats** in the header's job
line, with the rig and how far it is: `Measuring flats · 12 of 40
sessions`, then the pairs it compares, then the lights it reads.

- **When it runs.** After an import, a sync, an upload, a forget, a
  use/exclude choice or a duplicate repair, for the rigs whose calibration
  frames changed, and only for their sessions that have no current
  measurement. New lights alone ask only whether lights shot during a
  change sit before or after it. A burst of imports or uploads shares one
  pass: it starts 5 seconds after the last, once grading of the new frames
  is done. At start the server picks up only unfinished or stale work, so a
  catalog that is up to date reads no pixel.
- **Unfinished work.** Flats on a share that is away, lights the image
  folders do not show yet, or a catalog busy with another writer leave the
  rig to be tried again: after a minute, then less often, up to every half
  hour.
- **Stop** pauses measuring. Frames says so, and it resumes when those
  flats change (an import, sync, forget or keep-out choice), with **Resume
  now**, or when the server starts again. New lights alone do not resume
  it.
- **Settings.** A changed dust or light-pattern limit regroups from the
  stored numbers at once, without grading again. A rig switched from **By
  time only** to **By measurement** is measured then, and only that rig.
  A rig that groups by time is never measured.
- **Priority.** It uses the background share of the CPU, waits while a
  preview, a stack or other work you started runs, and keeps each result as
  it goes. While **Calibration › Frames** shows one rig's flats, that rig is
  measured first, with workers from the background share; nothing else
  waits for it.

A read-only catalog, or one from an older build, has no place for the
numbers and groups flats by time, as before.

### Response families

The rotator angle a flat records is a good guide but not proof of its
light path. A rotator re-homed or a camera re-seated reads the same angle
for another position, and on some optics flats tens of degrees apart
measure alike. So flats are grouped by what they measure, with the angle
as a starting point:

1. Within one rig, filter, focal length and sensor mode, the recorded
   angles first form **angle clusters**: angles join while the gaps between
   them are no wider than the rig's rotation tolerance, at most 2°. Flats
   at 101.3°, 102.2° and 104.0° are one cluster; flats at 82° and 104° are
   two. A wider match tolerance lets a light reach further for flats; it
   does not make two angles one.
2. Each session is compared with its neighbours in time in every other
   cluster of its filter, as well as within its own.
3. Two clusters join into one **response family** when some pair of their
   sessions shot within one flat session window, or neighbours in time,
   measure alike, and no pair shot within one window measures apart.
   Clusters never measured alike stay apart.
4. Flats that record no angle join the family one of whose neighbours in
   time they measure alike with, when exactly one does and every other
   was measured and is unlike. Alike with none, or with several, they keep
   to themselves at no angle.

A family's **measured angle range** is the recorded angles of its
clusters: `100.1–104.4°`, or `72.6° & 82.2°` for two angles whose flats
measure alike. Frames names each family by it.

### Groups and changes

Within one response family, sessions join into **groups** in time
order. Two sessions are alike when their dust and light
pattern are both within the limits: 1% dust and 2% light pattern by
default. Each group holds only sessions measured alike with every other
session it was compared with, so a slow drift cannot chain nights that
differ into one master.

Between two groups lies a **split**, a window from the last flat before it
to the first flat after it. Splits of one rig whose windows overlap and
whose shadows sit at the same sensor position are one **change**. Dust on
the camera side turns with the camera, so the same shadow shows in every
filter and every light path: a change seen in two filters, or in two
response families of one filter, applies to every light of the rig. A change seen in one filter, as filter
dust, applies to that filter only. One night unlike the nights either side
of it, which measure alike, as after a hazy dawn, keeps to a group of its
own but is no change: no light is flagged or kept from flats for it.

Two changes stay two. A filter with no flats between two changes splits
once across both. When another filter's flats show a group between them,
with a split on either side, and the long split shows the shadows of
both, the long split dates neither change: it makes no change of its own
and joins neither, though it still counts as one more filter that saw
each. Two changes never collapse into one with a window of a few minutes.
A change whose window is shorter than an hour, one flat run, says so in
Frames: it landed between one twilight's flats, or two changes met
there.

### Which side a light is on

A light stands before or after a change:

1. **By time**, when it was shot outside the change's window.
2. **By its imaging session**, when it was shot inside the window but the
   continuous run of lights it belongs to (one N.I.N.A. profile, no gap of
   three hours) reaches outside it. Only a run of the light's own rig
   counts: another rig imaging the same night says nothing of it.
3. **By its own pixels**, when the whole run lies inside the window. The
   stage takes the median of up to eight of the run's lights, divides it by
   a flat from one side, and fits what is left against the change's map,
   one shadow at a time. Each shadow's fit is weighed against the same fit
   at places around it where nothing changed, clear of the change's other
   shadows, and the shadows are then weighed together. Over a smooth field
   this decides, however far apart the shadows lie. Over a nebula it is
   usually undecided. A session whose lights could not be fitted, as when
   no map spans the change alone, is kept as undecided and read again
   only when the change's map or the flat it divides by changes.
4. Otherwise it is **undetermined**.

### Which family a light takes

First, the response family. The candidates are the families of the
light's filter and sensor mode whose measured angle range lies within 10°
of the light's angle:

- **Its angle** as it recorded it, and as the rotator's **epochs** turn it.
  N.I.N.A. records each light's sky angle, synced from its own plate
  solves, beside the mechanical angle; where PSF Guard has solved the
  light's pixels, that solve gives the sky angle instead. Sky − mechanical
  is constant until the rotator is re-homed or the camera re-seated, so
  each run of lights with one offset is an epoch, and a light compared
  with flats of another epoch is turned by the difference.
- **One candidate**: its flats.
- **Two or more**: the light's own pixels choose. The measuring job takes
  the sky background of up to eight of the imaging session's lights and
  fits it against what tells each pair of candidates apart, weighed
  against the same fit where nothing differs (Seiza's
  `flat_response::choice`). The family the lights follow wins when it
  stands clear of every other. Over a nebula they usually cannot tell, and
  the light takes the family nearest its angle; the panel says so. The
  answer is kept per imaging session and filter, so choosing flats reads
  no pixel.
- **None** within 10°: flats are matched by angle as before.

Within its family a light takes the flats at its own angle first, then
those within 10° of it, then the rest; the family's far angles serve
where nothing nearer lies on its side of a change. A light whose flats sit at another angle than it recorded is read
at its flats' angle when a stack judges it against its masters, so the
stacker does not refuse the master its family chose.

### Which flats a light takes

A light no change touches takes the nearest flats of its family, as
before. Otherwise, in this order:

1. Its family's flats from its own side of every change.
2. When it has none, its own side's flats at the nearest other angle, if
   this rig's flats measure alike across angles. Angles that measure alike
   are usually one family already; this serves a family never measured
   against the others. Flats of one filter shot
   at two angles within one flat session show how much the angle matters;
   that difference must be within the split limits.
3. Otherwise, when the change is on the camera side, its own angle's flats
   from the other side, carried across the change with the change's map:
   later ÷ earlier, measured from a pair of sessions in the light's
   filter, or another filter when its own has none. Only a pair with no
   other change between them serves, and a light takes one correction at
   most: flats from across two changes are kept as shot, and flagged. The
   corrected master is written beside the original and reused.
4. Otherwise its own angle's flats from the other side, and the light is
   flagged: no flats from before (or after) the change at its angle.

Flats that record no angle serve the angle they were measured at. Flats
whose angle is still unknown are a last resort: a light that records an
angle takes them only when no flat of a known angle matches it, or when
they are the only flats on its side of a change. Grouping by time leaves
both as before, where a missing angle matches any. A light whose flats
change this way takes a new master: its stack's key changes.

An undetermined light keeps the nearest flats and is flagged. The light's
Calibration panel says which of these chose its flats, under **Chosen
by**, with the change and how the light was placed. When its family was
not simply the one at its recorded angle, **Flat family** names it with
its measured angle range and how it was chosen: *chosen by its own
pixels*, *the only family near its angle*, *its angles measure alike*,
or *nearest angle*, with whether its lights could not tell or have not
been read yet. When its flat master
mixes nights, the panel says how alike they measured.

An export, and a WBPP run, place the same flats a stack takes. WBPP
cannot carry flats across a change, so such a light keeps its own
session with the flats as shot and is named in the runner and the export
summary; see [WBPP](WBPP.md#calibration-frames).

### Flat groups in Frames

Flat groups show where the flats are, in the Calibration page's
**Frames** view, whenever the list can hold flats (**Type** is All or
Flat):

- Each flat session in the list carries its group, such as **Group 3 ·
  09-07 → 09-16, 4 nights**, numbered from the oldest within its filter
  and angle, with a coloured band down its row's edge. Neighbouring
  groups take neighbouring colours. A session too small to measure, or
  not measured yet, says so.
- A **split** between two groups of one filter and angle is a line in the
  list above the newest session of the older group. It names the filter
  and angle and says why, with the values and the limits: *the light
  pattern changed by 2.6% (limit 2%)*.
- A **change** on the camera side reaches every filter and angle of the
  rig, so it shows once, as one row above the newest flat shot before it:
  what it is, its depth, the filters and angles it was seen in, and its
  window, with a note when the window is narrower than one flat run. Its
  splits do not repeat in each filter.
- **Compare**, on a split or a change, opens the ratio of the two
  sessions above the session view: later ÷ earlier, at ±3%, ±1% or
  ±0.5%, with new shadows and departed ones circled. Stars draw at
  neutral grey. **Compare** again, or **Close**, closes it.
- Above the list, a line for each rig in view gives its limits and how
  much its flats depend on the rotator angle, counts the sessions not
  measured, shows `Measuring flats · 12 of 40 sessions` while the job
  measures a rig in view, and ends in
  **Edit limits**. That opens **Settings › Calibration** on the dust
  limit, or on **Group flats** while the rig groups by time: the rig's
  own value when it sets one, else the global one.
- The chosen flat session's title says which group it is in, its nights
  and flats, and how it measured against the night before.

**Angle** narrows the list to flats shot within 2° of one rotator angle.
Like the other filters, the angle and the two sessions compared live in
the address. A link to the old **Flat groups** tab opens Frames on flats
with the same rig, filter, angle and comparison.

### Browse by group

**Sessions | Groups** at the top of Frames' sidebar lists the flat groups
themselves instead of sessions. It is there whenever the list can hold
flats (**Type** is All or Flat) and the rigs in view have measured groups:
offered only when Type is Flat it would hide from someone browsing All,
where groups already mark the sessions, and offered whenever the list
holds flats it would open on nothing for a rig grouping by time. The
address keeps the choice as `calBrowse=groups`.

The Groups list holds one entry per group, by rig and by family (one
filter at one angle, named from the measured sessions: **G · 3.2°**, with
the night it begins when two families share that name). Groups run newest
first, so the list reads as a timeline, and the split between two
neighbours sits between them with its reason: **Change · new dust 1.6%**,
**Split · pattern 2.6%**. Each entry is marked:

- **Current**: the family's newest group, the one a light shot now takes.
- **Partly measured**: a flat session of its filter and angle within its
  nights (or after them, for the current group) is not measured, or has
  too few flats to measure.
- **Change after**: a change follows it, its own split or a change of the
  whole rig after its newest flats.

↑ and ↓ move between groups, as in Images and Sequence. **Filter** and
**Angle** narrow the list; the other filters are about frames and leave
it alone.

Choosing a group shows it in the main column the way a session is shown:
its name, then its flats in the same image browser, the strip of small
pictures with the chosen one large beside its numbers. The strip holds
every flat of the group's nights, oldest first, and marks where each
night begins with its date and how many flats it holds, in the group's
colour. ← and → step from flat to flat across the nights while ↑ and ↓
keep moving between groups; the chosen flat lives in the address as
`calFrame`, beside `calGroup`. Choosing another group starts at its
first flat, and a group none of whose flats the filters keep shows no
frame at all, so Keep out and Forget never reach the last group's.
Switching between Sessions and Groups hands each list back the frame it
last showed. In **Compare with session** the comparison and the zoom hold
as the strip crosses into another night. The strip holds up to 5,000
flats; a longer group says it shows only the first of them. The page's
filters narrow the strip as they narrow a session's frames; a project
scope does not, since a group is the same whoever uses it. Below the
browser:

- **Members**: its nights, oldest first, as cards. A card brings the
  strip to that night's first flat; **Open session** under it opens the
  session in the session list, on the page that holds it, where a filter
  that hides the session gives way, then the project scope.
- **Why they belong together**: the most any two of its nights measured
  apart in dust and light pattern, against the rig's limits, over every
  pair measured; each night against the next and the pairs furthest apart,
  with bars against the limits; and **Edit limits**.
- **Its edges**: the split or change on each side, with **Compare**, the
  same ratio picture as a split in the session list.
- **What it feeds**: the recorded flat masters with sources in it, each
  linking to Masters, and the lights whose flat master it is, by project.
  A light whose master takes flats of two groups counts in both; one
  calibrated with a master from other software counts in none. A
  project's line opens Images on exactly those lights. Lights shot during
  a change whose side is unknown (undetermined), lights given its flats
  from another angle, carried across a change, or from the far side of
  one are counted apart, and the flagged ones (undetermined, or every flat
  from the far side) have a link of their own. The counts come from the
  light headers the calibration check keeps; until it has read them all,
  the page says how many it has not, and how many lights could not be
  read or found. The server works the counts out in the background, never
  in a request: until the first answer the section says it is working
  them out, and while a newer one is worked out it keeps the last counts
  and says **Updating…**, rather than saying the group feeds nothing.

The address names the group as `calGroup=<key>`. The key is the first,
by id, of the sessions holding the group's oldest night, so it survives a
regroup that keeps that night, and a key naming any session of a group
opens that group: a link still lands after nights join or split. When one
session holds flats of two families, as a night shot at two gains may,
each group's key also carries a digest of its family's key. A key that
names no group's session any more, as when every flat of that night was
kept out, opens the group of the family that session's flats would join
(the same rig, filter, gain, frame size, binning and focal length class)
at the nearest angle, around its night. A Groups link holds the sidebar at **Loading groups…** until
the groups (and a project's scope) are in, rather than showing the
session list for a moment.

**Images on one group's lights.** A project's line in **What it feeds**
opens Images with `flatGroup=<db>:<key>` (or `…:flagged` for the flagged
link) beside the database and project. The grid then keeps exactly the
lights the group counts, by id: not other rigs' lights, other angles' or
other filters' lights shot the same nights. A **Lights of a flat group ✕**
chip heads the toolbar, the summary line says **Lights of one flat group**,
and the filter counts as one of the Images filters; the chip's ✕ or
**Clear** removes it. A value made in another database does nothing and
is not counted. Until the lights are known the grid shows none, so that
Select all, Accept and Reject reach nothing outside the group, and says
*Finding the group's lights…*. A key that names no group any more, as
after its nights joined an older group, says **This flat group no longer
exists** with **Show every light**; when the lights cannot be found the
grid says so and asks again after a growing wait.

In a project scope, Groups lists the groups holding the project's
sessions, each with its neighbour on either side, dimmed and marked
**Neighbour**: a group's edges are where its flats stop being right, and
the group across an edge is what Compare sets it against. A group the
scope holds only because a correction's map is measured from it, and
whose flats none of the project's lights take, is dimmed too and marked
**Correction map**, once every light of the catalog has been read and none
is unreadable. Groups further
off are left out, and a gap row counts them. ↑ and ↓ keep to the
project's own groups. **✕** on the chip widens to the rig's whole
library, still in Groups.

On a phone the sidebar stacks above the group, a short scrolling picker,
as the session list does.

### Settings and checks

Three settings sit under **Settings › Calibration › Flat groups**, global
or per rig (**Edit limits** in Frames leads there): **Group flats** (by measurement, or by time only as before),
**Dust that splits flats** and **Light pattern that splits flats**. They
are matching settings: a change rebuilds the masters and stacks whose
flats it changes.

**Flats from across a change** (`check_flats_differ`) raises Check when
a light's flats are all from the other side of a change at its angle and
nothing can stand in, or when the light was shot during a change and
nothing tells which side it is on. **Flats from several nights**
(`check_mixed_flats`) now fires only when a master mixes nights that were
not measured against each other; nights measured alike mix without a word.

### Flat groups API

`GET /api/db/{db}/calibrations/flat-groups` returns each rig's limits,
angle difference, changes, families, groups, splits and unmeasured
sessions. `GET /api/db/{db}/calibrations/flat-groups/ratio?earlier=&later=&stretch=`
answers `202` while the picture is queued and the PNG once it is ready;
`POST /api/db/{db}/calibrations/generation-status` reports on it with
`{"target": "ratio", …}`. In a light's calibration explanation the flat
kind gains `chosen_by`, `basis` and `alike`. Each group carries its `key`.

`GET /api/db/{db}/calibrations/flat-groups/use` returns, by group key, the
recorded flat masters with sources in it and the lights whose flat master
it is, by project and by how their flats were chosen, with how many
lights the calibration check has not read yet (`pending`) and how many
could not be read (`unread`). `group=<key>` adds that group's lights by
id (`image_ids`, `flagged_ids`) for the Images filter. No request works
the answer out: one that finds none, or an older one, starts a single
background refresh and gets what is kept, marked `refreshing`, with
`computed` false until the first answer exists. A refresh that fails is
logged, and its `error` is sent with the answer; the same revision is
tried again only after a wait that doubles from 10 seconds to 10
minutes. With `group`, `group_found` says whether the key names a group
now, by its own key or one of its sessions or response keys, and
`group_key` gives that group's key. The refresh matches
lights from the headers the calibration check keeps, on a read-only
connection of its own and a pool sized at background priority that waits
while interactive work runs. The answer is kept until something matching
depends on changes: a frame, its validity mark or whether it is kept out,
the recorded masters, flat grouping's measurements, the settings, any
project's lights or name, or a project's kept headers. Grading the
library again in the background changes none of these unless it keeps a
frame out or lets one back in: the health pass that follows writes a
project's headers again only when they differ, so their stamp stays.
`GET …/calibrations/flat-groups/place?session=` gives the family, night
and angle a flat session's flats would take, measured or not. Each group in `flat-groups` also carries
`widest`: the most any two of its sessions measured apart, and over how
many pairs; `pairs` holds each session against the next and the widest.
`calibrations/sessions?locate=<session>` lists the page holding that
session and says whether the filters keep it (`located`).
`calibrations/frames?group=<key>` lists a flat group's flats, the group
found by its key or one of its sessions or response keys as the page finds
it (none for a key no group answers to), within a project scope when one
is named. They come session by session in night order (`sort=night`), a
flat with no capture time with its session, in pages of up to 5,000. Each
listed frame says whether it can be compared with its session
(`compare`).

`project=<id>` on `calibrations/sessions`, `calibrations/frames`,
`calibrations/masters` and `calibrations/flat-groups` keeps what that
project's lights use (see [Project scope](#project-scope)).
`GET /api/db/{db}/calibrations/project-scope?project=<id>` returns the scope
itself: its sessions, masters and rigs; how many lights were matched, could
not be read, or are still being read (`pending`); its `revision`, which
changes with what it was worked out from; and `refreshing` while a newer one
is on its way.

`psf-guard flat-groups CATALOG --work DIR` measures a catalog on a copy: it
opens the catalog read-only, copies it into `DIR`, writes responses and
comparisons there, and prints the groups, splits, changes and angle
difference. `--images` finds lights for the pixel evidence, `--lights`
says how each light's flats would be chosen, `--map-prefix FROM=TO`
rewrites calibration paths recorded on another machine, and `--report`
writes it all as JSON. It never writes the catalog or a frame.

See the [flat grouping design record](design/flat-grouping.md) for the
metric, keys and the validation on real flats.

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
it, and each session counts its frames by grade and those kept out. With an
editor account, each frame offers:

- **Keep out**: leave the frame out of masters and exports whatever its
  grade, as rejecting does for a light. It is offered for every frame,
  including one its grade already keeps out: kept out by hand, it stays out
  if a later grade would let it back in. Unlike **Forget**, the frame stays
  in the library, so a rescan does not bring it back. The frame shows
  **Kept out by hand**.
- **Use anyway**: put a frame the library keeps out back in use.
- **Clear**: let the grade decide again.

The choice survives regrading, rescans and database copies. A frame kept out
counts for nothing in [calibration health](#calibration-health): a suspect
frame kept out no longer makes the lights it would have served worth a
check.

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
column, which older PSF Guard builds read. While some dark in a master's set
has no grade yet, master building applies the same rule to its own inputs,
which catches a dark the job has not reached yet. Once every dark in the set
is graded or chosen, it leaves the grades to decide, so **Use anyway** puts a
dark back into its master. A master's record lists only the frames it
combined.

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
| Flat | bias fields, filter, telescope, focal length within 1 mm, and rotator angle within 2° |

The temperature, dark reach, focal length and angle are the defaults; see
[Tune the checks](#tune-the-checks) to change them for every rig or one.

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
  (**Settings › Calibration › Light checks**, default 60 days; 0 lifts it).

Each of the three is a check that can be made advisory or turned off,
for every rig or one (see [Tune the checks](#tune-the-checks)); only a gap
whose check flags it makes a light one that cannot be calibrated.

Each rule applies only where the library could have filled the gap, so a rig
that never shoots flats, or never shoots darks, is not affected. The report
lists each night whose lights cannot be calibrated. **Check lights**, at the
foot of the report, reads every light of the project that is not already
rejected and lists those that cannot be calibrated, by night, filter and
reason. **Reject N lights** then sets them Rejected with the reason, so
Target Scheduler shoots them again. It refuses, and asks for a new check,
when the list changed after it was shown.

Stacks keep such lights unless **Settings › Calibration › Stacks and exports ›
Leave out lights that can't be calibrated** is on. With it on, and calibration
on or automatic, stack previews leave them out, so they never mix with properly
calibrated frames: the stack card counts them under **can't calibrate** and
each frame's decision gives the reason. Every export leaves the same lights
out, and says how many and why: both export layouts, WBPP runs, the CLI, and
the AstroBin acquisition export (see [Export](#export)).

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
- Its flat master takes flats from nights not measured against each other
  (see [Flat groups](#flat-groups)).
- Its flats are from across a measured change, such as new dust, with
  nothing to stand in, or it was shot during the change and nothing tells
  which side it is on.
- Its masters use frames graded suspect.
- Frames kept out by hand leave its bias, darks or flats under five frames.
- Its bias master would use fewer than five frames while a dark serves it,
  or a few changing spots sit in most of its flats.

The thirty days, the five frames and the spot limits are defaults, and each
finding comes from a check that can be made advisory (Check at most) or
turned off; see [Tune the checks](#tune-the-checks).

A light takes its worst finding, and a project its worst light. The Library
shows a **🧪 Poor** or **🧪 Check** pill after the grading and date pills,
on the compact row and the card; it opens the Calibration report, whose
**Quality** section lists each finding with its lights by night and filter.
**Show › Calibration issues** narrows the Library to those projects.

The image views mark the same lights. In Images, Sequence and a mosaic's
sequence, a **🧪** chip under the score chips is red on a Poor light and
yellow on one to check; hover it for its findings, or click it to open the
light on its **Calibration** panel. The chip has its own toggle beside the
🌙 and layers toggles, and in **Settings › Review**.

An image's detail view has a **Calibration** panel. It lists the light's
findings in words, worst first, as the library stands now, and for bias,
darks, dark flats and flats the frames each master would take: how many,
from which nights and at what sensor temperatures, the suspect frames it
takes, the frames kept out and why, the frames that match but lose to a
nearer set, and the frames refused with the reading that disagrees. Each
night opens those frames on the Calibration page. Choosing a finding picks
out the frames behind it, such as the two nights of a mixed flat master or
the three darks of a thin one, and folds the kinds it is not about; **Show
every kind** opens them all. The choice is kept in the address
(`calibFocus`), so it stays chosen from image to image and in a shared
link. Opened from the chip, or from Images filtered by a finding, the
panel comes into view with that finding chosen.

The panel folds like the others beside the image. It opens on its own for
a light the background check marked Poor or Check, and stays folded for one
with no finding, with "no findings" on its title line; the mark decides this,
so a folded panel asks the server for nothing until you open it. Opening or
folding it by hand makes that the rule for every light, kept in this browser.
Coming from the chip or from Images filtered by calibration opens it either
way; folding it then keeps it folded until you follow another chip or
filter.

Images has a **Calibration** filter: **Poor**, **Check**, or both. Sequence
adds **Poor calibration** to **Select**, so the lights can go straight to
**Review rejection**. In the Calibration report, each Quality finding, each
night that can't be calibrated, and each line of **Check lights** opens
those lights in Images, filtered to that finding and its filters.

Reading every light takes time, so this runs in the background, never on a
request: after each grading pass (which follows the server start, imports,
syncs, forgets and remote uploads), when a light's grade changes, and when a
frame is kept out or used anyway, a validity mark changes, or the
calibration settings change. A directory refresh and a change of image
folders start one too, since a light's file may have turned up or moved.
When the Library or an image view finds a result that no longer holds, as
after N.I.N.A. writes new lights or the CLI regrades some, it asks for one:
at most every two minutes per database, not while grading runs (grading asks
for one when it ends), and not for fifteen minutes after a pass failed.
It uses the background worker share and waits while interactive work runs;
a server that stops ends it between lights.

The results are kept below the database's cache folder, each filed under a
key of the library and its grades, the project's lights, the calibration
settings and the rules, and beside it where the project's lights were found
in the image folders. A project is read again only when one of those
changed, and then only the lights whose file changed are read: each light's
header is kept, with its file's size and modification time, from the last
read. Accepting a light, or setting it back to pending, changes no key;
rejecting one does. Until a project is read again, a result that no longer
holds shows no pill. The image views keep showing its marks, so rejecting
lights does not clear the rest until the pass ends; Sequence holds back
**Poor calibration** in **Select** until then.

## Tune the checks

Every limit on this page that decides which frames calibrate a light, what
the light checks flag, and where a calibration frame grades suspect or poor
is a setting under **Settings › Calibration**. The defaults are the limits
PSF Guard used before these settings existed, so a library nobody tunes
behaves exactly as it did. A rig whose workflow is sound but unusual, such
as one that shoots sky flats or cannot cap for darks, need not be flagged
for it.

### Where a value comes from

1. The light's rig, when **Settings › Calibration › Rigs** sets the value
   for that rig.
2. Otherwise the server's value, set in the same tab above the rigs.
3. Otherwise the default.

A rig is the calibration library's rig (see [Rig identity](#rig-identity)):
its telescope, camera, frame size and binning. Its key is a hash of those,
so one rig has one key in every catalog and its values follow it from
database to database. The header text is taken as written, lower-cased but
not trimmed, so a renamed N.I.N.A. profile or a stray space in `TELESCOP` or
`INSTRUME` makes another rig, which starts with no values of its own. A light's rig is read off its own header; a
calibration frame is graded by its own rig's limits. The **Rigs** list
shows every rig an open catalog records, with its databases and frame
count; a rig opened from a light's Calibration panel shows even when no
catalog lists it. Each row says whether a value is the rig's own or what
it inherits and from where; **Own value** sets or clears one, and
**Inherit everything** clears them all.

**Start from** fills a rig's form with a preset to review before saving:

| Preset | Sets |
|---|---|
| Sky flats | flat session 72 h, flats from several nights off, flat light pattern suspect 5% and poor 10%, flat too dim suspect 5% and poor 2% |
| Dark library | dark reach 730 days, no matching bias or dark advisory |
| No darks | use darks off |

### What a change rebuilds

- **Matching** settings change which frames a light's masters take.
  Masters are content-addressed by their frames, so a changed set is a new
  master. A stack's key takes the matching settings that differ from their
  defaults, so a setting that changes only which matching darks or flats a
  master keeps, such as the complete night or the flat session, still
  builds the stack again; a stack nobody tuned keeps its key. The dark
  temperature, flat rotation and flat focal length tolerances also decide
  which frames are graded against each other, so changing one grades the
  frames again too.
  When a stack calibrates a light it checks the masters against the light
  with the same tolerances, so a flat or dark matched by a wider one is
  used, not refused.
- WBPP exports and runs match the same way when they are planned, so one
  planned after a change takes the new frames (see [Export](#export)).
- **Light checks** change only what is flagged. The health pass judges the
  lights again from the headers it kept; nothing is rebuilt and no pixel is
  read.
- **Frame grades** run grading again on the stored measurements, without
  reading pixels. A frame that turns poor leaves its masters, which then
  build again; one that stops being poor comes back.

### Light checks

Each light check is **Flag**, **Check only** or **Off**:

- **Flag** raises the finding at its own level: Poor where the light cannot
  be calibrated properly, Check where it is worth a look.
- **Check only** shows the finding as Check. It never makes a light one
  that cannot be calibrated, so stacks keep it, **Check lights** does not
  list it, and the Library's pill reads Check at most.
- **Off** does not flag it. The light's Calibration panel still lists it
  under **Not flagged**, with who turned it off.

The panel also says whose settings checked the light (the global ones or
its rig's own), lists every limit that differs from its default with
where it came from, and words each refusal with the limit it crossed:
`dark is 3.2 °C warmer than the light (limit 2 °C, set for Askar 120 ·
ASI2600MM)`. An editor can open the rig's settings from there.

### Settings

| Key | Setting | Default | Range | Changes |
|---|---|---|---|---|
| `rotation_tolerance_deg` | Flat rotation tolerance | 2° | 0–180 | matching |
| `focal_length_tolerance_mm` | Flat focal length tolerance | 1 mm | 0–100 | matching |
| `dark_temperature_tolerance_c` | Dark temperature tolerance (darks and dark flats) | 3 °C | 0–20 | matching |
| `flat_session_hours` | Flat session: how far apart one master's flats may be | 24 h | 1–720 | matching |
| `use_darks` | Use darks | on | on, off | matching |
| `external_masters` | Masters from other software | use whenever one matches | prefer, fallback, ignore | matching |
| `dark_reach_days` | Dark reach | 183 days | 1–3650 | matching |
| `complete_dark_frames` | Complete night | 10 darks | 2–64 | matching |
| `dark_choice` | Dark choice | complete set first | complete, nearest | matching |
| `flat_grouping` | Group flats | by measurement | measured, time | matching |
| `flat_split_dust_pct` | Dust that splits flats | 1% | 0.1–10% | matching |
| `flat_split_pattern_pct` | Light pattern that splits flats | 2% | 0.2–20% | matching |
| `flat_max_age_days` | Flat age limit | 60 days | 0 (no limit)–3650 | light checks |
| `flat_check_age_days` | Flats worth a look after | 30 days | 0 (off)–3650 | light checks |
| `thin_master_frames` | Thin master below | 5 frames | 2–64 | light checks |
| `check_no_library` | No frames for its camera | flag (Poor) | flag, advisory, off | light checks |
| `check_no_flat` | No matching flat | flag (Poor) | flag, advisory, off | light checks |
| `check_no_bias_or_dark` | No matching bias or dark | flag (Poor) | flag, advisory, off | light checks |
| `check_stale_flats` | Flats past the age limit | flag (Poor) | flag, advisory, off | light checks |
| `check_thin_dark` | Thin dark master | flag (Poor) | flag, advisory, off | light checks |
| `check_thin_bias` | Thin bias master | flag (Poor without a dark, Check with one) | flag, advisory, off | light checks |
| `check_flat_spots` | Changing spots in flats | flag (as the spot limits grade them) | flag, advisory, off | light checks |
| `check_old_flats` | Old flats | flag (Check) | flag, off | light checks |
| `check_mixed_flats` | Flats from several nights not measured against each other | flag (Check) | flag, off | light checks |
| `check_flats_differ` | Flats from across a change | flag (Check) | flag, off | light checks |
| `check_suspect_frames` | Suspect frames in its masters | flag (Check) | flag, off | light checks |
| `check_short_after_keep_out` | Short after keeping frames out | flag (Check) | flag, off | light checks |
| `grade_bias_level` | Bias level, in the set's noise | suspect 1, poor 3 | 0–1000 | frame grades |
| `grade_bias_noise` | Bias noise, ratio to its set | 1.1, 1.3 | 0–100 | frame grades |
| `grade_bias_banding` | Bias banding, ratio to its noise | 0.25, 0.5 | 0–100 | frame grades |
| `grade_dark_level` | Dark stray light, in its spread | 0.5, 1 | 0–1000 | frame grades |
| `grade_dark_noise` | Dark noise, ratio to its peers | 1.1, 1.3 | 0–100 | frame grades |
| `grade_dark_hot_pixels` | Dark hot pixels, ratio to its peers | 1.5, 3 | 0–1000 | frame grades |
| `grade_dark_banding` | Dark banding, ratio to its noise | 0.25, 0.5 | 0–100 | frame grades |
| `grade_flat_clipped` | Flat clipping | 0.05%, 0.5% | 0–100% | frame grades |
| `grade_flat_bright` | Flat too bright | 80%, 92% | 0–100% | frame grades |
| `grade_flat_dim` | Flat too dim (lower is worse) | 10%, 3% | 0–100% | frame grades |
| `grade_flat_illumination` | Flat light pattern | 2%, 5% | 0–100% | frame grades |
| `grade_flat_spots` | Flat spots | 100, 500 | 0–100000 | frame grades |
| `grade_flat_changing_spots` | Flat changing spots | 3, 20 | 0–100000 | frame grades |
| `grade_flat_carried_spots` | Spots carried into a flat master | 1, 10 | 0–100000 | frame grades and the changing-spots check |

A grade limit left empty is never reached. **Mask stars in flats** and
**Leave out lights that can't be calibrated** stay server-wide.

### What stays fixed

Some limits are the stacker's, and loosening them would only hand it frames
it refuses:

- A dark of another exposure than its light (beyond 0.05 s or 0.1% of the
  longer) is refused when the stack calibrates.
- The frames of one master are compared with its first: within 1 °C, and a
  flat within 2° and 1 mm. A wider flat tolerance lets a light take flats
  further off, but one master's flats still agree with each other that
  closely.
- Camera, frame size, binning, gain, offset, readout mode, Bayer layout,
  filter and telescope must agree; they are identity, not tolerance.
- The integrator's own stray-light screen, which applies only while some
  dark of a master is not graded yet, uses its default limit.
- The **Masters** tab's **Thin** badge and changing-spots note describe the
  master itself, which may serve lights of several rigs, so they keep the
  defaults.

### API

`GET /api/settings/calibration` returns every setting's range and default
(`fields`), the server's values (`rules`) and each rig with its own values
(`rigs`). `PUT /api/settings/calibration` takes `rules` by key: a key left
out keeps its value and `null` restores the default.
`PUT /api/settings/calibration/rigs/{rig_uuid}` replaces a rig's own values
(`{"name": …, "rules": {…}}`), and `DELETE` on the same path lets it
inherit everything. Viewers read; changing takes an editor account. Every
value is checked against its setting: an unknown key, a value of the wrong
kind, out of range, or a grade whose suspect limit comes after its poor one
is refused, naming the setting.

The registry keeps the values in its `calibration` block, the server's
beside the keys older builds read and each rig's under `rigs`. At most 256
rigs can have values of their own. A setting goes into force only once the
registry is saved, so a save that fails changes nothing.

An older build reads the keys it knows. If it saves the registry, it drops
`rigs` and every key it does not know, so after a downgrade the rigs' values
and the new global ones must be set again.

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

Matching follows the settings in force for each light's rig (see [Tune the
checks](#tune-the-checks)); the CLI reads them from the registry. The flat
rotation, focal length and session limits decide which flats land in a light's
session folder, the dark settings decide which darks go and whether any do, and
a master from other software goes only when its setting uses it.

With **Leave out lights that can't be calibrated** on, every export leaves out
the lights a stack would, in either layout and from the dialog, the server
export directory, a download or the CLI: those whose gap is flagged. A check
set to **Check only** or **Off** lets them go. Each export says how many it
left out, with one and its reason: the export's summary, the CLI's output, a
`LEFT-OUT-UNCALIBRATED.txt` note listing every one in a download, and the WBPP
runner scripts.

The runner scripts also list the settings that differ from the defaults and
the rigs that export no darks. An export into a folder that already holds
an earlier one keeps the earlier files, and WBPP scans each frame folder whole,
so the runner warns when it finds frames this export did not place; remove
them or export to an empty folder.

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
wins. The preview counts the choices a transfer brings, since each changes
what the destination's masters are built from. When both machines cataloged
the same file under different IDs, the destination's choice stays with the
file. They do not copy generated master rows or cached FITS files because
those paths belong to the source machine. The destination
resolves a stale raw path by basename in its configured image folders, then
builds its own master.

As with other transfer work, the UI requires a dry preview before apply.
