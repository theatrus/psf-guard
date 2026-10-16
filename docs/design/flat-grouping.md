# Flat grouping

Status: **Built**. Still open: split and join by hand, moving
`flat_groups::measure` into Seiza, gap lights (a light whose night has no
flats between two groups that differ), and corrected flats in WBPP
exports (§8).
Last updated: 2026-10-15

User-facing behaviour is in
[Calibration libraries › Flat groups](../CALIBRATION_LIBRARY.md#flat-groups).
This record explains the measurements, the rules built on them, and the
keys that keep caches honest.

## 1. Problem

A flat master may take flats from more than one night, and a light may
take flats from a night when the optical path was different. Before this
work, PSF Guard judged both by the clock alone:

- Every master that mixed nights raised **Check · mixed flats**, whether
  or not the nights agreed. On the catalogs checked, every such mix
  measured alike.
- Nothing noticed when a light's flats came from the far side of a dust
  change. On UltraCat131, new dust arrived between 2026-09-17 and
  2026-09-21. The Heart of the Soul R lights of 2026-09-18, at rotator
  113.8°, took the only R flats at that angle, from 2026-09-21, and carried
  a +1.07% ghost of the new shadows with no warning.
- The build-time stability check counted bright pixels as features, so
  tracked sky flats with stars that stay put were set aside as "dew or
  frost" (23 RedCat61 master records).

## 2. Measurement

All of it lives in `src/calibration/flat_groups/`. The measurements sit in
one module, `flat_groups::measure`, with a pure API: planes and numbers
in, plain structs out, no catalog, server or file types. Its `metric`
holds the response, the star mask, the comparison (`Q`, dust, broad,
spots), the change map and applying it to a flat; `model` holds groups,
change events, the angle term and placement; `evidence` the light fit;
`rows` the data they share. A test fails if any of them names `crate::`,
`rusqlite` or the file system, so the module can move to Seiza as it
stands (§8). Only the background job (`job.rs`) reads pixels.

### 2.1 Responses

A **session** is one night (at the catalog's night boundary) of one
family, with every frame's rotator angle within 2° of the first. A
**family** is one rig, filter (case-folded), focal length (to the
millimetre), frame size, binning, gain, offset and readout-mode name; a
reducer swapped in at the same gain is another family, never new dust.
A catalog measured before the key took the focal length has its rows
renamed on the next pass; nothing is read again.

Rotation clusters are formed on load, by single linkage: the angles of a
family's sessions, around the circle, split only where the gap between
neighbouring angles is wider than the rig's rotation tolerance
(`rotation_tolerance_deg`, 2° by default). On C925, sessions at 100.1° to
104.4° are one cluster: no gap between them reaches 1°. Grown from the
lowest angle instead, 104.00° and 104.22° fell into two clusters and were
never compared. The family's angle is its middle session's.

A session whose flats record no angle, on a rig whose other flats of that
family do, is compared with its neighbours in time in every cluster
(`Book::pairs_to_compare`) and joins the cluster one of them measures
alike with, the closest when several do; its flats then serve lights at
that angle (`Session::inferred_rotation`). Unlike all of them, it stays in
a family of its own, at no angle. The pass makes comparisons in rounds,
at most three, so a session that joined a cluster is compared within it
in the same pass.

A session with at least 3 flats in use (not graded poor, not kept out) gets
a **response** (`metric::build_response`):

1. Take at most 16 flats, evenly spaced in capture order.
2. Read each through `image_io`, sum 2×2 CFA cells on a colour sensor, and
   bin by block means until the long side is at most 1,024 px.
3. Divide each by its median; take the per-pixel median, clipped at 3σ.
   Stars that move between frames drop out.
4. Keep the plane, the session's raw level, and the noise per response
   pixel.

The pedestal is the median level of the rig's matching dark-flats, else
its bias frames, else 0, from the stored frame measurements. It is kept on
the response row and taken out when the plane is used
(`FlatResponse::without_pedestal`), so the plane on disk does not depend on
it.

### 2.2 Comparison

`metric::compare(earlier, later, bin)` divides the later response by the
earlier one, `Q`, renormalized to 1. A light shot in the later state and
divided by the earlier flats carries `Q − 1` as a fraction of its sky
background. Every number below is in that unit.

1. **Stars.** A response pixel more than 5σ above the upper quartile of its
   9 × 9 neighbourhood is a star; the mask grows by 3 pixels. Masked pixels
   take no part in what follows. Dust only takes light away and is never
   masked.
2. **Broad.** A robust degree-4 polynomial over 32 block medians across,
   three clipping rounds, gives the smooth surface `S`. Its best plane is
   removed, since background extraction removes a straight gradient. `broad`
   is the largest `|S/P − 1|` inside a 2% border.
3. **Dust.** `H = Q/S − 1`, less a median over a window one sixth of the
   long side (sky gradients that bend into a corner), smoothed by one
   pixel. A spot is a connected region over 5σ and at least 200 sensor
   pixels. A spot counts only when one of the two responses holds a dip
   there deeper than any bump the other holds: **dust is dark**. `dust` is
   the largest spot peak. A dark and a bright spot within three radii are
   one moved shadow.

Comparisons are stored without limits applied. Limits apply on load, so a
changed limit regroups without reading a pixel.

### 2.3 Defaults

Set from every flat of two Starfront rigs (§7):

| Setting | Default | Why |
|---|---|---|
| `flat_split_dust_pct` | 1.0% | Nights with nothing moved peak at 0.46% (an L-filter reflection that changes with the dawn sky); UltraCat131's new dust measures 1.57–1.60%. |
| `flat_split_pattern_pct` | 2.0% | The per-frame light-pattern suspect limit within one session; real pairs within a family peak at 1.05%, and 1.94% against one hazy dawn. |

## 3. Groups and change events

`model::Book::build` works everything out from the stored rows on each
library load, memoized per catalog by a digest of the rows, the rules, the
profile-to-rig bindings and a summary of the lights taken inside SQLite
(their count, last id, and the sums of their times and profile lengths),
so matching one light does not read every light to find the book
unchanged.

- **Pairs.** Within a family, every pair within twice the flat session
  window (the furthest one master can reach), every pair of neighbours,
  and every pair of next-but-one neighbours.
  Across rotation clusters of one filter, pairs shot within one window:
  these give the rig's **angle term**.
- **Groups.** Agglomerative, in time order, complete linkage: the
  neighbouring groups with the smallest largest distance merge while it is
  at most 1, where `d = max(dust / dust limit, broad / broad limit)`. A
  slow drift cannot chain nights that differ into one group.
- **Boundaries.** Between two groups: a window from the last flat before
  to the first flat after, the step's dust and broad, its spots, and a
  drift note when no single step crossed the limit.
- **Outliers.** A group of one session between two groups whose facing
  sessions measure alike is one odd night, such as a hazy dawn, not a
  change: its two boundaries still split groups but make no event, so no
  light is flagged or kept from flats for it.
- **Events.** Boundaries of one rig whose windows overlap and whose spots
  coincide merge. The window is the overlap. An event seen in two filters
  or at two angles is **camera side** and applies to every light of the
  rig; otherwise it applies only to its filter.
- **Two changes are not one.** A merge is refused when some family of
  the rig holds a group with a split on either side, both splits inside
  the span the merged boundaries reach over (first flat to last, give or
  take one flat run), and the members show the shadows of both splits
  (a spot at least half as deep as each one's deepest; a comparison
  lists faint spots down to 5σ, which say nothing of a shared shadow):
  that family saw two changes there, and some member spans both. On main
  before this rule, R at 104° had flats in May and on 08-25 only, and its
  one split, carrying both corner shadows, merged with the Aug→Sep change
  while OIII at 104° held a group from 08-18 to 08-24 between the two.
  The merge made a five-minute window on 08-25. A merge of the splits
  that saw the donut arrive is not refused for the group that saw it
  leave a day later: that group's second split ends outside the span.
- **Narrow windows.** An event whose window is shorter than one flat run
  (`FLAT_RUN_SECONDS`, an hour) is marked `within_flat_run`: the change
  landed between the flats of one twilight, as a filter wheel can shake
  dust loose, or two changes met there. The view and the report say so.

## 4. Lights

### 4.1 Placement

`Book::place(event, time)`:

1. **Time**: outside the window, the side is known.
2. **Session**: lights of one profile with no gap over 3 hours form an
   imaging session; if the session reaches outside the window, the side is
   that end's. A session places only lights of the rigs its profile is
   bound to (`psf_guard_rig_binding`; an unbound profile counts as the
   library's only rig, when it has one): another rig imaging the same
   night says nothing of this one.
3. **Pixels**: the job reads the imaging sessions that lie wholly inside a
   window (`evidence.rs`). Per filter: the clipped median of up to 8
   lights, less the pedestal, divided by the nearest flat response with a
   known side, fitted against the change's map. Stars are masked by their
   cores only: a few dithered lights keep their stars, and grown as far as
   in a flat they would cover most of a star field. The map's support is
   split into **shadows**, its connected pieces (pieces within 3 response
   pixels are one shadow), and each shadow is fitted on its own. Its fit
   is repeated at placebo positions, the shadow moved by whole multiples
   of its own size; a shadow with fewer than six such places (too large,
   or pressed into a corner) takes them on rings around it at 1, 1.5 and
   2 times its longer side instead, each at least half inside the frame.
   Each weight is taken against its placebos' median, and their spread is
   1.4826 × the median absolute deviation. The shadows' weights combine
   by inverse variance (no spread below 0.02); when they disagree beyond
   their spreads, the combined spread grows by the Birge ratio. A side is
   chosen when the weight lies within two spreads of it and at least
   three from the other, with a spread of at most 0.3. Filters that
   decide must agree. Over smooth fields this decides; over nebulae it
   usually does not, and it never takes the wrong side in the tests. The
   pedestal must come off first: left in, it damps the change and every
   light looks like it predates it. Before shadows were fitted apart, a
   change of four shadows spread from x 191 to x 6,108 on C925 left the
   whole template no room to move, and every fit was undecided
   (`EVIDENCE_ALGORITHM_VERSION` 2).
4. **Undetermined** otherwise.

Pixel evidence is evidence about the lights; a comparison between flats is
evidence about the flats. The account names which placed the light.

### 4.2 Choosing flats

Before any change is looked at, the angle gate learns what grouping
measured. Seiza's `rotation_matches` takes an unknown angle as a match
for any, for rigs without a rotator. A flat that records no angle but
whose session joined a rotation cluster is matched at that cluster's
angle. One whose angle stays unknown is a last resort
(`LibrarySnapshot::keep_unknown_angles_last`): a light that records an
angle takes it only when no raw flat of a known angle matches. On C925,
the 2026-08-18 dusk flats record no angle. Measured, the Hα and SII ones
match the rig's 11.1° flats (0.90% and 0.96% dust) and differ from its
102° flats by 3–4%, so they join 11.1°, and lights at 102° that night no
longer take them. B, G and OIII have no 11.1° flats of their own to
measure against: their 08-18 flats stay of unknown angle, and the 96
Bubble Nebula lights at 11.1° that took them still do, as the last
resort, since nothing else matches. Grouping by time keeps Seiza's rule.

`LibrarySnapshot::choose_flats_around_changes`, after the usual gates and
nearest-first ordering, for every event that applies to the light:

1. Keep the candidates on the light's side of every event whose side is
   known. When the dropped flats could never reach the light's master
   (the nearest flat is on its side with a second within one window, and
   every dropped flat is further than two windows away), the list stays
   as it was, so the stack keeps its key, and the dropped flats are held
   back (`CalibrationSelection::flat_held_back`): no fallback may take
   them, neither the next set when the nearest is unstable nor an
   external master.
2. None left at its angle: the nearest other angle on its side
   (`other_angle_flats`), when the rig's angle term is measured and within
   both limits.
3. Otherwise, for a camera-side event whose two sessions' responses are
   on disk: the light's own flats, carried across by the change's map
   (`metric::change_map`), from the event's boundary in the light's filter
   or its strongest member (`Book::map_source`). Only a member whose
   window no other change of the rig falls in may serve: one that spans
   two changes would carry both. On C925, SII at 104° split once from
   05-19 to 09-15, across the donut as well as the corner shadow; borrowed
   for 30 NGC 7331 L lights whose 08-24 flats already showed the donut, it
   would have applied it twice. The member must also show the state the
   light's flats are in (`Book::map_source_for`): no other change that
   reaches the member's filter may fall between the flats and the
   member's session on their side. One map carries flats across one
   change: a light whose nearest flats lie across two keeps them and is
   flagged, and a change whose side the nearest flat is already on leaves
   the choice to the change that keeps it from the light. With no clean
   member the light keeps the other side's flats and is flagged, as in 4.
   The built master is divided by the map for a
   light before the change and multiplied for one after
   (`corrected_flat_master`), written as `<master>-across-<key>.fits` with
   a `PSFGACRS` card, to a temporary file with the build counter and
   renamed. A correction that then fails fails the light's masters rather
   than serving the uncorrected master under the corrected key. The
   master cleanup counts it as recorded and rebuildable while its source
   master is.
4. Otherwise keep the other side's flats and flag (`NoSameSide`).

An undetermined light keeps the nearest flats and is flagged. A light
undetermined for one change and placed for another keeps its Check, with
the other change's choice (its flats, gate or correction) inside the
undetermined basis (`FlatBasis::followed_by`). The basis
travels on `CalibrationSelection::flat_basis` into the explanation
(`chosen_by`, `basis`) and the health check.

A stack judges each light against its masters under a gate
(`StackOptions::match_tolerances`, `LiveStacker::set_match_tolerances`,
seiza-stacking 0.34): its rig's tolerances, from #839, carried on the
light's `CalibrationSession` and set before each session's pushes and in
the final pass. A light given another angle's flats gets a gate whose
rotation is just wide enough for them, the angle between the light and the
flats plus the rig's own (112.6° for the Heart of the Soul lights), with
every other field the rig's (`CalibrationSelection::match_tolerances`).
The widening is part of the session key, so every light of a session
shares its gate; the stack key does not need it, since the other angle's
flats already change the light's candidates. A flat of another filter,
focal length or angle than the one chosen is still refused. A corrected
master keeps its source master's headers, the light's own angle, and
passes under the rig's gate.

**Seiza's `residual_flat`** was checked before writing the correction and
the light evidence. `build_residual_flat_patch` estimates an
attenuation-only patch, bounded at a 1.2× gain, from repeated crops of
lights where the caller has shown a feature is fixed on the detector.
Neither job here fits it:

- The correction is measured from two flat sessions, which carry far less
  noise than lights; it may brighten as well as darken (dust that left or
  moved); and a light after a change multiplies by it. A patch only
  divides out a dip.
- The evidence asks whether lights carry a known map, not what they carry.
  Over a nebula, structure that barely moves between dithers would pass
  the patch's consensus as a fixed feature; the placebo fit measures that
  confusion instead.

`residual_flat` remains the tool for removing dust that no flat holds.

### 4.3 Exports and WBPP

Every export plans through `plan_export`, which matches each light with
`select_for_light`, so an exported light's flats are its event-aware
selection. On top of that:

- **Sessions.** A light and its flats share a `SESSION_<night>` folder.
  Lights of one target, filter and night that take different flats get a
  session each, `<night>-<digest of the flats>`, so WBPP builds each its
  own master. Nights whose flats a master pools, measured alike, share
  the earliest night's session.
- **Other angle.** The other angle's flats are exported in the light's
  session; WBPP does not compare rotator angles.
- **Corrected.** WBPP cannot apply a change map, and building the
  corrected master during an export would make planning build masters.
  The light takes the flats as shot in a session of its own,
  `<night>-across-<key>`, and is listed in the runner's notes, the export
  summary (`flat_warnings`), the WBPP run's progress and a
  `FLATS-ACROSS-A-CHANGE.txt` beside a downloaded export.
- **Flagged** lights (no same-side flats, or undetermined) are listed the
  same way.
- **Notes.** The runner names each change in force with its window and
  counts the lights each fallback served.
- **Run identity.** A server WBPP run plans its frames when it starts and
  writes them into its own folder, so a run started after a regroup takes
  the new flats; nothing caches a run by its frames.

### 4.4 Validation of the order

On the Heart of the Soul R lights (§1), measured in the lights themselves
as the footprint against a ring around it:

| Flats | Error |
|---|---|
| Today's: 09-21 R at 113.8°, after the dust | +1.07% ghost |
| Same side, nearest angle: 09-17 R at 3.2° | the angle term only: 0.28% dust, 0.71% broad on the flats |
| Corrected: 09-21 ÷ the R change map at 3.2° | +0.05% |

The other angle comes first: its error is measured from the rig's own
same-night pairs. The corrected flat is slightly better here but depends
on a map shot close in time to it; a map from Hα 19 days later left 0.75%.

## 5. Flags and settings

- `check_flats_differ` (`flats_differ`, Check only): `NoSameSide` and
  `Undetermined` lights.
- `check_mixed_flats` (`mixed_flats`): only masters that mix nights not
  measured against each other (`Book::measured_alike`).
- `flat_grouping` (`measured` | `time`), `flat_split_dust_pct`,
  `flat_split_pattern_pct`: matching settings in the **Flat groups**
  group, global or per rig, under PR #839's scheme. `time` restores the
  previous selection exactly.

### 5.1 On the Calibration page

Flat groups have no tab of their own. They show in **Frames**, where the
sessions are laid out by night, whenever the list can hold flats
(`docs/CALIBRATION_LIBRARY.md` › Flat groups in Frames). The view
(`GET …/calibrations/flat-groups`) names, for each measured session and
each unmeasured one, the Frames sessions that hold its flats
(`sessions`), so the page joins the two by id rather than by night and
angle; a flat-group session and a Frames session agree except where a
rotator drifted across their different angle rules. Each rig names the
flat-group settings it sets itself (`own_limits`), so **Edit limits**
opens the rig's own value or the global one.

- A **split** draws above the newest listed session of the older group,
  found by group membership, so it sits right on any page of the list.
- A **camera-side change** draws once, above the newest listed flat of
  the rig shot before its window; its member splits do not draw again.
  A filter-side change is a split.
- The page polls the quality job's progress once, through
  `useFlatGroups`, and reads the view again when a pass that worked on
  flats ends.
- `calRotation` is a Frames filter (`rotation` on the session and frame
  routes, flats within `ROTATION_CLUSTER_DEG`); the facets list the
  flats' angles. An old `calTab=groups` link becomes `calKind=flat` and
  keeps its other params.

## 6. Storage, keys and work

- **Tables**, created with the calibration tables and on open of a
  writable catalog, never in a read-only one:
  `psf_guard_flat_response`, `psf_guard_flat_comparison` and
  `psf_guard_flat_light_evidence`. A catalog without them groups by time.
  They do not travel in a sync: the keys use machine-local fingerprints.
- **Response key**: the response algorithm version, the size cap and the
  sorted `frame_uuid:source_fingerprint` of the frames combined. Planes are
  `<cache>/<db>/calibration-flat-responses/<key>.resp`, 16-bit fixed point
  (× 32,768), written to a temporary file and renamed. Rows and planes of
  sessions no longer in use are removed at the end of a pass.
- **Comparison key**: both response keys and the comparison algorithm
  version. A pass takes each session's pedestal again; when it moved, as
  when a dark-flat arrived, the row keeps the new one and the session's
  comparisons are made again. A correction's key carries both pedestals.
- **Forgets and twins**: forgetting a frame, or merging a twin row into
  the row kept (the duplicates repair, or a sync that honours one), drops
  every response built from those rows with its comparisons
  (`store::forget_frames`); the next pass measures the session again from
  the rows the library holds.
- **Partial reads**: a session measured while some of its flats could not
  be read is measured again on each pass until it combines all of them. A
  session whose flats could not be combined at all stays recorded with its
  error until its frames or the algorithm change its key.
- **Evidence key**: the imaging session (its profile and first light),
  the event and the evidence algorithm version. A change's key is its rig
  and window, so new flats that narrow the window ask the lights again.
- **Stacks**: `selection_hash` adds `flat-correction=<key>` for a
  corrected flat. Which flats a change leaves a light already shows in its
  candidate list.
- **Health**: `library_digest` hashes the three tables;
  `HEALTH_ALGORITHM_VERSION` is 4.
- **Stability check**: dips only; `FLAT_STABILITY_VERSION` 2, and
  judgements from version 1 are made again.
- **Pictures**: the ratio picture goes through the preview queue
  (`GenKind::FlatRatio`), keyed by both responses, the stretch and the
  render version, and reports through the shared generation-status poll.
- **Job**: a stage of the calibration quality job after grading, on the
  background worker share sized by the library's largest flat
  (`plan_workers` with its samples), yielding to interactive work before
  each read, comparison and evidence session, committing as it goes.
- **Grouping by time** is the previous behaviour throughout: selection,
  the mixed-flats Check and its wording, export sessions (no per-flat-set
  sessions), and the light's account, which says nothing of measurements.
  On copies of UltraCat131 and RedCat61 its selection and health match
  main's light for light; the account only adds the setting to its notes,
  as for any setting off its default.

## 7. Validation on real flats

`psf-guard flat-groups` ran on read-only copies of two catalogs, reading
frames from the archive in place:

- **UltraCat131** (ZWO ASI6200MM Pro, 9,576 × 6,388): 47 sessions, 32
  comparisons, in 90 s. One event, 2026-09-17 12:27 to 2026-09-21 12:28
  UTC, new dust near (1,423, 3,545), 1.60%, camera side, seen in B, G, Hα,
  L and R. Splits at 3° of 1.57–1.60% dust with at most 0.14% broad. Angle
  term from 7 same-session pairs: at most 0.28% dust and 0.95% broad.
  The 2026-09-17 run's own NGC 7023 B and G lights place it before the
  change, so its 23 Heart of the Soul R lights at 113.8° take the
  2026-09-16 R flats at 3.2°. The 84 lights shot during the change over
  nebulae (Sh2 119, Heart of the Soul Hα and OIII) stay undetermined and
  get the Check.
- **RedCat61** (ZWO ASI2600MM Pro, tracked twilight sky flats): 136
  sessions, 102 comparisons, no split, no change and no flag; stars
  masked.
- **C925** (ZWO ASI2600MM Pro behind a rotator that wanders from 100° to
  104°): 118 sessions, 216 comparisons, 7,872 lights. Three camera-side
  changes: a window donut and new shadows between 2026-05-20 and the
  first flats that show them, 08-11 13:05; the donut gone by 08-12 13:06;
  a corner shadow between 08-25 and 09-16. Before these fixes the first
  window ran to 08-19, two corner-shadow changes met in a five-minute
  window on 08-25, all 94 fits over the donut were undecided (no room for
  placebos), and 3,054 lights were flagged. Now 37 of 87 fits decide and
  1,463 lights are flagged: 913 shot during a change with nothing to
  place them, 550 whose flats lie across two changes. The 1,045 Question
  Mark Galaxy lights of May are placed before the donut by their own
  pixels. The same rerun leaves UltraCat131 (one change, 84 lights
  flagged, 23 at the other angle) and RedCat61 (no change) as they were.

The prototype behind these numbers, its pictures and the method are kept
with the PR's working notes; the Rust metric reproduced them.

## 8. Follow-ups

### Move to Seiza

Once the current calibration work lands:

- **`flat_groups::measure` into `seiza-calibration`**: the session
  response, star mask, comparison metrics, change map and its
  application, event detection, placement and the light evidence. The
  module is already pure (§2), so it lifts without rework; PSF Guard keeps
  the job, the store and the selection.
- **The stability check onto Seiza's measurement.** PSF Guard's
  `flat_set_stability` reads each flat back against its master and, until
  this change, counted bright departures too (`abs()`), which made tracked
  sky flats look like dew. Seiza's `measure_flat` already finds dips only
  (`FlatFeature` is "one compact dip"), so the check can be built on it
  and leave PSF Guard.
- **Caller tolerances on light validation.** Done in seiza-stacking 0.34
  (seiza #270): a stack's gate and `validate_light_frame_with` replace the
  fixed `MatchTolerances::default()` (3 °C, 2°, 1 mm) that capped #839's
  dark temperature setting and refused the other-angle fallback.

### PSF Guard

- **Split and join by hand**, synced as choices.
- **Gap lights**: a light whose night has no flats, between groups that
  differ, takes the nearest by time without a word.

- **Corrected flats in WBPP exports**: export the corrected master itself
  once exports can build masters.
