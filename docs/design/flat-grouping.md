# Flat grouping

Status: **Built**, with flats matched by measured response (§2.3, §4.2).
Still open: split and join by hand, gap lights (a light whose night has no
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

The pixel measurements are Seiza's, `seiza_calibration::flat_response`
(seiza #282): `metric` holds the response, the star mask, the comparison
(`Q`, dust, broad, spots), the change map and applying it to a flat;
`evidence` the light fit against a change; `choice` the light fit against
several candidate responses. PSF Guard re-exports them as
`flat_groups::measure::{metric, evidence, choice}` and keeps, in the same
module, what it builds on the numbers with a pure API: `model` (response
families, groups, change events, the angle term and placement), `lights`
(angle epochs and which family a light takes) and `rows` (the data they
share). A test fails if any of those names `crate::`, `rusqlite` or the
file system. Only the background job (`job.rs`, `family_job.rs`) reads
pixels.

### 2.1 Responses

A **session** is one night (split at the rig's local noon; see
[Nights](../CALIBRATION_LIBRARY.md#nights)) of one
family, with every frame's rotator angle within 2° of the first. A
**family** is one rig, filter (case-folded), focal length class, frame
size, binning, gain, offset and readout-mode name; a reducer swapped in at
the same gain is another family, never new dust. Focal length classes
(`job::focal_classes`) chain a rig's recorded focal lengths while each is
within the coherence tolerance one master's flats are held to (1 mm by
default) of the next; a flat with none joins the rig's only class, or is
a class of its own when the rig has several. A catalog measured before
the key took the focal length has its rows renamed on the next pass;
nothing is read again.

Rotation clusters are formed on load, by single linkage: the angles of a
family's sessions, around the circle, split only where the gap between
neighbouring angles is wider than the rig's coherence tolerance for
rotation: `rotation_tolerance_deg`, capped at 2°. The match setting may
be widened to 180° so lights reach further for flats; that must not chain
0° and 90° into one family. On C925, sessions at 100.1° to
104.4° are one cluster: no gap between them reaches 1°. Grown from the
lowest angle instead, 104.00° and 104.22° fell into two clusters and were
never compared. The family's angle is its middle session's.

A session whose flats record no angle, on a rig whose other flats of that
family do, is compared with its neighbours in time in every cluster
(`Book::pairs_to_compare`). It joins a cluster only when exactly one
measures alike and every other was measured and is unlike, and takes the
angle of the neighbour it matched, not the cluster's middle; its flats
then serve lights at that angle (`Session::inferred_rotation`). Alike
with none, or with several, as on optics where the angle barely matters,
it stays in a family of its own, at no angle. The pass makes comparisons in rounds,
at most three, so a session that joined a cluster is compared within it
in the same pass.

A session with at least 2 flats in use (not graded poor, not kept out) gets
a **response** (`metric::build_response`). Two flats make a master, so two
make a response; a session of one stays out:

1. Take at most 16 flats, evenly spaced in capture order.
2. Read each through `image_io`, sum 2×2 CFA cells on a colour sensor, and
   bin by block means until the long side is at most 1,024 px.
3. Divide each by its median; take the per-pixel median, clipped at 3σ.
   Stars that move between frames drop out.
4. Keep the plane, the session's raw level, and the noise per response
   pixel: the per-frame spread of neighbouring frames' differences, over
   √n. With fewer than three flats (`metric::MEDIAN_FRAMES`) the
   combination is a mean, not a median, and keeps half of anything that
   moved; a comparison that takes such a session sets two of its own
   spreads (`model::NOISY_DUST_SIGMAS`) against its dust before the limit
   (`Limits::noisy_distance`), since the deepest spot of a noisier ratio
   stands that much higher for noise alone.

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

### 2.3 Response families

The recorded rotator angle is a prior, not an identity. On C925 the
offset between N.I.N.A.'s synced sky angle and the rotator's mechanical
angle takes at least five values: re-homes moved the mechanical angle of
one framing by up to 20°. Of 54 same-filter flat pairs shot within 36 h,
every pair 2–5° apart measured alike, and three pairs over 5° apart did
too (OIII 82.2° and 72.6°, OIII 140.9° and 81.2° twice).

`Book::build_families` forms, per rig and family key (rig, filter, focal
class, sensor mode):

1. **Rotation clusters** by single linkage, as before (§2.1).
2. **Response families** by joining clusters, closest first
   (`Book::join_alike_clusters`): two join when some pair of their
   sessions shot within one flat session window, or neighbours in time,
   measures alike, and no such pair measures apart (`Book::family_link`).
   Complete linkage over the pairs that speak to the angle, so a family
   never chains: Hα at 80° alike with 140° three days later, and 140°
   apart from 80° three days after that, are two families, however alike
   the two 80° nights are. A pair apart is let pass only when a change of
   dust explains it (`Book::apart_by_a_change`): a session at one of its
   angles measured alike with the other and apart from the one beside it,
   as a morning's two angles alike after new dust and the same angle
   apart from the morning before. Clusters never measured alike stay
   apart.
3. **No-angle sessions** join the family one of whose neighbours they
   measure alike with, when exactly one does and every other was measured
   and is unlike (`Book::measured_cluster`), as before, now against
   families.

`Book::pairs_to_compare` asks for each session against its neighbours in
time in every other rotation cluster of its family key, so every session
has a measured neighbourhood whatever its header says (C925: 216
comparisons became 436).

**Alike ranges** (`Book::build_alike_ranges`, `model::AlikeRange`): a
family's own angle term, from pairs of its sessions compared directly.

- **Evidence**: pairs at two angles further apart than the rig's
  tolerance, in one group, with no change of the rig that reaches the
  filter between them. Only measured pairs count; a header's angle never
  makes or widens a range.
- **A range** gathers the alike pairs whose two ends each lie within the
  tolerance of one pair's ends. It needs `ALIKE_RANGE_MIN_SESSIONS` (2)
  distinct sessions on `ALIKE_RANGE_MIN_NIGHTS` (2) nights at each end:
  one session at an end can make a range alone (a gradient of its
  twilight, a header that changed while the rotator did not), so pairs
  sharing it do not count twice.
- **Serving** (`AlikeRange::serves`): a light at L may take flats at F
  only when one pair's own arc, within the tolerance at either end,
  holds both, and L and F lie no further apart than that arc plus the
  tolerance, which so counts once. A chain of pairs never bridges angles
  no pair compared: 100°↔104°↔108° alike does not hand a 100° light 108°
  flats. Ranges may overlap, sharing an end, and stay apart.
- **Veto**: a pair of the family measured apart that a range would serve
  rules the range out.
- **Across changes** (`Book::carries_angle_term`, `Family::carry_class`):
  a range is built per group, and may gather pairs from, and serve flats
  of, other groups of the family only across changes that are
  camera-side dust: seen in two filters or two families, with spots, and
  no broad change over the limit. The argument: how flats depend on the
  angle comes from the telescope side (vignetting, reflections, the
  light path through a rotating camera's optics); dust on the camera's
  window or filter moves with the camera, adds the same shadows at every
  angle, and leaves that dependence as it was. A broad change
  (vignetting), a change seen in one filter alone (filter or telescope
  side), a split that makes no change, or a split made by hand carries
  nothing (a seam is left for #878's choices).
- **What it brings back**: this carrying is what serves C925's lights
  shot before the 05-20 dust their own side's 05-19 flats at 104.0°: no
  pair at two angles was shot before that change, so the ranges come
  from flats after it. The data can test the argument only across the
  later changes. Hα's and OIII's ranges hold pairs on both sides of the
  09-16 corner shadow, each side alike on its own, so the angle term
  survived that camera-side change; across the 05-20 change itself it is
  an inference from the argument, not a measurement.

A family's **measured angle range** is its clusters' recorded anglesA family's **measured angle range** is its clusters' recorded angles
(`Family::angles`, `Book::angle_label`): `100.1–104.4°`, or
`72.6° & 82.2°`. Its `rotation` is the middle angle of its largest
cluster. Its key is the family key and its earliest session's first flat, so a
session that arrives later never renames it, nor does a new night
boundary, and a choice made by hand (§8) can find it again. Groups and splits within a family work as before.

### 2.4 Defaults

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
  drift note when no single step crossed the limit. In a family of two
  angles the step is told by the nearest pair across it at one angle;
  a boundary with no such pair is at two angles, splits groups, and
  makes no event (`Book::at_two_angles`): the light path differs there as
  well as the dust, and which changed cannot be told.
- **Outliers.** A group of one session between two groups whose facing
  sessions measure alike is one odd night, such as a hazy dawn, not a
  change: its two boundaries still split groups but make no event, so no
  light is flagged or kept from flats for it.
- **Events.** Boundaries of one rig whose windows overlap and whose spots
  coincide merge. The window is the overlap. An event seen in two filters,
  or in two response families of one family key (one filter, optics and
  sensor mode, two light paths), is **camera side** and applies to every
  light of the rig; otherwise it applies only to its filter. Two angles
  of one family are one light path and count once.
- **Splits across two changes.** A split is **spanning**
  (`Book::spanning`) when another family of the rig holds a group with a
  split on either side, both splits inside the split's window (give or
  take one flat run), and the split shows the shadows of both. It dates
  neither change: it makes no event and joins none, so it cannot demote
  a change to one filter or leave a change months wide that no map can
  serve. Each event inside its window whose shadow it shows still counts
  its filter and angle toward the camera side. A shadow is shown when a
  spot of the split's, at least half as deep as its deepest (a comparison
  lists faint spots down to 5σ), sits in the other at least half as deep.
- **Two changes are not one.** A merge is likewise refused when some
  family holds such a group inside the span the merged boundaries reach
  over, and the members together show the shadows of both splits. On main
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
   of its own size, never onto the change's other shadows; a shadow with
   fewer than six such places (too large, or pressed into a corner) takes
   them on rings around it at 1, 1.5 and 2 times its longer side instead,
   each at least half inside the frame.
   Each weight is taken against its placebos' median, and their spread is
   1.4826 × the median absolute deviation. The shadows' weights combine
   by inverse variance (no spread below 0.02); when they disagree beyond
   their spreads, the combined spread grows by the Birge ratio. A side is
   chosen when the weight lies within two spreads of it and at least
   four from the other, with a spread of at most 0.3. Filters that
   decide must agree. Over smooth fields this decides; over nebulae it
   usually does not. Thirty nebulae of 2–8% structure, one shadow and
   four, 480 fits each: at three spreads from the other side two fits of
   each took the wrong side, both with spreads near 0.3; at four, none
   did, and about a third still decide. A sweep is not a proof, so a
   decision over a nebula remains evidence, not certainty. The
   pedestal must come off first: left in, it damps the change and every
   light looks like it predates it. Before shadows were fitted apart, a
   change of four shadows spread from x 191 to x 6,108 on C925 left the
   whole template no room to move, and every fit was undecided
   (`EVIDENCE_ALGORITHM_VERSION` 2). A session whose lights were read but
   could not be fitted (no map spans the change alone, no reference) is
   stored as undecided, so it is not read on every pass. Its key
   (`Book::evidence_key`) names the change and, per filter of the rig,
   the map's two sessions and the reference flat, so a borrowed map or a
   reference that changes asks the lights again. Each row also records
   the change it answers for (`change_key`). At the end of a pass a row
   goes only when it is of an older fit, when its imaging session is gone,
   or when the pass asks for the same session and change under another
   key. A row whose change a limit removed stays, even while its session
   sits in another change's window: on C925, raising the dust limit to 3%
   removed every change, and putting it back to 1% read all 46 sessions'
   lights again until this rule.
4. **Undetermined** otherwise.

Pixel evidence is evidence about the lights; a comparison between flats is
evidence about the flats. The account names which placed the light.

### 4.2 Choosing flats

**The family first** (`LibrarySnapshot::flat_family`,
`Book::choose_family`, `measure::lights`). With measured grouping and a
light that records its angle:

1. **Compatible families**: those with a flat that passes every gate for
   the light but the angle (`flat_family_compatible`).
2. **Angle epochs** (`epochs.rs`, `lights::AngleEpochs`): per rig, each
   light's sky − mechanical offset, modulo a half turn, from N.I.N.A.'s
   `RotatorPosition` and `RotatorMechanicalPosition` in
   `acquiredimage.metadata`. Where PSF Guard has a pixel solve of the light
   that still matches its file (`astrometry::source_still_matches`), its
   position angle gives the sky angle instead, once the rig's lights that
   have both agree on a convention (a constant turn, either sense, scatter
   at most 1°). The measuring job gathers the offsets
   (`flat_groups::gather_angles`) into `psf_guard_flat_light_angle`,
   re-reading a solve only when its file's stamp changed; the book reads
   that table and never the cache.
   A jump of more than 2.5° between consecutive lights starts an epoch:
   N.I.N.A.'s re-syncs move the offset by a degree or two (RedCat61
   wanders between 138.6° and 140.6°), re-homes and re-seats by several
   (C925: 76°, 79°, 87°, 97°).
   A light compared with a flat session turns by the difference of their
   epochs' offsets (`AngleEpochs::shift`).
3. **Candidates** (`Book::family_candidates`): compatible families with a
   session within `ANGLE_PRIOR_DEG` (10°) of the light's angle, recorded
   or turned.
4. **One candidate**: it (`Angle`), only when it has flats at the light's
   recorded angle, within the rig's tolerance. An alike range never
   counts as the recorded angle; a header or an epoch alone never hands a
   light another angle's flats.
5. **Two or more**: the light's imaging session's own pixels, when the
   pass has read them. They **confirm** the family at the light's
   recorded angle freely (`FamilyBasis::Pixels`, `moved: false`). They
   **move** the light to another family only with corroboration
   (`lights::corroboration`): the rotator's epochs turn the light into
   that family's angles, or another filter's lights of the same imaging
   session, at the same angles, moved to a family at the same angles too.
   A move nothing corroborates is `Disputed`: the light keeps the family
   at its recorded angle, if any, and is flagged (Check, `FlatsDiffer`).
   Unread, or undecided, the family at the light's recorded angle
   (`Nearest`, `undecided` when read); with none there, no family.
6. **None**: matched by angle as before.

A family serves the light's flats when it is at the light's recorded
angle or its pixels chose it. Then its flats match: flats of another
family with an angle do not, nor unmeasured flats when its pixels moved
the light to another angle (`LibrarySnapshot::family_admits`). A flat of
the family at another angle than the light's is admitted only when the
light's pixels chose the family, when its session measured alike with
a session of the family at the light's recorded angle
(`Book::alike_with_angle`), or when an alike range of the family, of the
flat's own carrying class, serves both angles and the flat lies on the
light's side of every change of its rig that reaches its filter, the
light's side known for each (`Book::alike_range_for`,
`LibrarySnapshot::on_lights_side_of_changes`, which asks
`flat_groups::place_light`, the one placement hook selection uses). A
light shot while a change happened takes nothing through a range. The
stacker's gate then reads the light at the flats' angle, and the light's
`FlatFamily::alike_range` names the range, its pairs and nights for the
panel ("Flats at 104.0°: this family's flats at 101.5–102.2° and
104.2–104.4° measure alike (10 pairs over 9 nights, measured across a
change of dust on the camera side only)").

Flats admitted at another angle than the light's (by its pixels, by a
measured-alike session or by a range) stay candidates only where they can
do something: when no flat at the light's own angle lies on no known
other side of a change from it, and only those on no known other side
themselves. A stack's key names every candidate flat
(`selection_hash`), and its master may fall back on the next set, so
the candidates must stay; but a light whose own angle serves it keeps
exactly the candidates, and the stack key, grouping by time gives it. Within the family, flats at the angle the
light recorded sort first, as before families: measured alike is not
identical; then those within the angle prior, then the rest, each in
time order. The family's far angles serve where nothing nearer lies on
the light's side of a change. The epochs widen the candidates but never
reorder a family's flats: a sync's degree or two would.

**Light evidence for families** (`family_job.rs`, stage
`STAGE_FAMILIES` of the measuring job, per rig in its scope after the
change evidence): the lights of every imaging session are grouped by
filter, target and angle cluster (single linkage at the rig's rotation
tolerance), and each group with two or more candidates is read once, up
to `evidence::MAX_LIGHTS` lights. A light takes the row of its session
and filter whose time and angle range hold it. A group whose kept row is
current is skipped without reading its headers. Each
candidate stands as its session nearest in time among those within the
prior of the lights' middle angle, turned into that session's epoch
(`Book::family_representative`): a family that is a candidate through
one epoch's session is weighed by that session, not by one shot where the
rotator stood elsewhere. Pedestals come out; the
background is fitted against every pair (`choice::choose_response`, both
ways round, with placebos), pairs measured alike flat against flat
(`Book::measured_alike`) not fitted. A candidate wins when it beats every
candidate it can be told apart from and loses to none; several winners
alike with each other give the nearest angle among them, but never when
any pair could not be fitted. The row (`psf_guard_flat_family_evidence`)
keeps the candidates, the choice and each pair's fit; selection reads no
pixel.

**The stacker's gate.** A light whose master's flats lie off its
recorded angle by more than the rig's tolerance has its gate read it at
their angle (`MatchTolerances::light_rotation =
LightRotation::Measured { degrees }`, seiza #282;
`family_light_rotation`), so the stacker accepts the master its family
chose. The flats' angle is that of the master's coherent subset, so every
light of one master shares the gate, and `session_key` carries it
(`flat-light-rotation=`).

**Then the angle gate** learns what grouping measured. Seiza's `rotation_matches` takes an unknown angle as a match
for any, for rigs without a rotator. A flat that records no angle but
whose session joined a rotation cluster is matched at that cluster's
angle. One whose angle stays unknown is a last resort
(`LibrarySnapshot::keep_unknown_angles_last`): a light that records an
angle takes it only when no raw flat of a known angle matches, or when it
is alone on the light's side of the changes the light is placed against
(else the light would be corrected from flats before a change when its
own side's flats exist). Lights on rigs that mix flats with and without
angles can take different flats than before, so their stack keys change
and their masters rebuild. On C925,
the 2026-08-18 dusk flats record no angle. Measured, the SII ones match
only the rig's 11.06° SII flats (0.96% dust) and differ from its 82° and
102° flats by 2.6–3.2%, so they take 11.06°. The Hα ones match both 11.1°
(0.90%) and 7.8° (0.91%), so their angle stays unknown. Lights at 102°
that night no longer take any of them. B, G and OIII have no 11° flats to
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
- The page polls the flat measuring job's status once, through
  `useFlatGroups` (`GET …/calibrations/flat-measure`, the header's poll of
  the same catalog), shows
  its progress while it measures a rig in view, and reads the view again
  when a pass ends. With one rig chosen it also sends `POST
  …/flat-measure/watch` (§6).
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
- **Family evidence**: `psf_guard_flat_family_evidence`, added like the
  other tables, by session key, canonical filter, target, angle cluster
  (its low angle to 0.1°) and `FAMILY_EVIDENCE_VERSION`; a table of the
  older shape is dropped and made again. Its key
  (`Book::family_evidence_key`) digests the candidates' family keys in
  order, the sessions and responses standing for them, which pairs of
  those measured alike, and the evidence and Seiza choice algorithm
  versions, so new flats nearer the session, families that joined, a
  comparison that changed or a new choice ask the lights again. A pass drops rows of the rigs it judged that no question asks
  for, and rows of imaging sessions the catalog lost; other rigs' rows
  wait for their own pass. Lights read but nothing to fit keep a row that
  chooses nothing; lights not found leave the rig to be tried again.
- **Light angles**: `psf_guard_flat_light_angle`, one row per light (rig,
  time, offset, the solved position angle and its file stamp), written by
  the job only when a row changes.
- **Book digest**: the family evidence and light angle rows join the
  digest the book is memoized by; loading the book reads no directory.
- **Stacks**: `selection_hash` adds `flat-correction=<key>` for a
  corrected flat. Which flats a change leaves a light already shows in its
  candidate list.
- **Health**: `library_digest` hashes the three tables and the family
  evidence; `HEALTH_ALGORITHM_VERSION` is 5, as families change which
  flats a light takes.
- **Stability check**: dips only; `FLAT_STABILITY_VERSION` 2, and
  judgements from version 1 are made again.
- **Pictures**: the ratio picture goes through the preview queue
  (`GenKind::FlatRatio`), keyed by both responses, the stretch and the
  render version, and reports through the shared generation-status poll.
- **Job**: `src/server/flat_measure.rs`, a tracked job of its own per
  catalog (see [background jobs](background-jobs.md)), no longer a stage of
  the calibration quality pass. One `job::run` plans the catalog once, then
  takes each rig in turn through responses, comparisons and evidence,
  committing as it goes. It reports `FlatMeasureStatus` (rig, stage, done
  of total) to the header's job line and the Frames page, and turns
  `active`, and shows, only once a stage finds work.
  - **Scope** (`job::Scope`): rigs whose flats changed get responses,
    comparisons and evidence; rigs whose lights changed get evidence only.
    Forget, a use/exclude override and duplicate repair name their rigs
    (the frames' `rig_uuid`, the repair plan's groups). Import, sync pull
    and peer pull diff each rig's raw calibration frames
    (`flat_measure::rig_state`: uuid, file fingerprint, kind, time, stray
    light, grade) before and after, on a connection of their own, never the
    request connection; import adds lights when it imported any. An upload
    names its frame's rig, or lights. Within a rig the pass reads only
    sessions without a current response, missing pairs and imaging
    sessions with a new evidence key.
  - **Retains** are per rig: a pass drops the stale response rows of the
    rigs it measured, never another rig's, then every comparison whose
    sessions are not both kept, then planes with no row. A rig in scope
    whose flats are all kept out (or gone) has no plan; its rows go too,
    unless it groups by time.
  - **Debounce**: requests merge, and a pass starts
    `FLAT_MEASURE_SETTLE` (5 s) after the last one, and only once the
    catalog's grading is not running or waiting for uploads to settle
    (`calibration_quality::busy`), since grades decide the flats in use.
  - **Retries**: a rig left with flats away, lights the folders do not show
    yet, or a pass that ended in an error (as a busy catalog), is tried
    again after `RETRY_FIRST` (1 min), doubling to `RETRY_MAX` (30 min), at
    most `RETRY_LIMIT` (6) times; a new change to the rig starts again.
  - **Stop** (`POST …/flat-measure/cancel`) ends the pass at its next step
    and pauses what ran and waited. A rig's stopped work resumes when that
    rig's flats change (an import, sync, forget, override or repair that
    names it), with **Resume now** on the Frames page (`POST
    …/flat-measure/resume`), or at the next start. New lights alone, as a
    remote upload, resume nothing, and neither does a page's look. A
    change that arrives after Stop but before the pass notices keeps its
    work in line rather than stopped.
  - **Start** and adding a database queue every rig: only unfinished or
    stale work is done, and a catalog that is up to date reads no pixel.
  - **Settings**: a change to `flat_grouping`, `flat_split_dust_pct`,
    `flat_split_pattern_pct` (or the window or rotation tolerance the
    limits carry) regroups on the next read, since the book's digest holds
    the rules; health is judged again, nothing is graded again. The rigs
    whose own limits changed and that group by measurement now are queued,
    or every rig when the server-wide limits changed; the pass reads only
    what the new grouping lacks, as for a rig switched from time. The rule
    books alone say which rigs: nothing is read on the request. A change
    grading reads queues every rig after the regrade, as before.
  - **Time mode**: a rig with `flat_grouping = time` is skipped at every
    stage; its rows stay for when it is switched back.
  - **Tiers**: a pass runs `Background` and waits for a person's
    interactive work before each step. While Frames shows one rig, the page
    sends `POST …/flat-measure/watch {rig}` every 20 s; for `WATCH_TTL`
    (45 s) after each, that rig goes first and runs `Viewed`: workers
    borrowed from the background share (the cores in use stay at the
    interactive and background shares), leased again for each step, and no
    interactive guard, so autoimport, grading, backfill and automatic stacks
    carry on and a stack the person starts plans with its whole share. A
    viewed pass does not wait out `RESUME_QUIET` after a person's work, as
    automatic stack refreshes do: a page is showing what it measures. At
    most every `WATCH_RECHECK` (5 min) a look also queues a check of the
    rig, which reads nothing when nothing is stale; a rig the catalog does
    not record is ignored.
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
  window on 08-25, 94 of 99 fits had no room for placebos, and 3,054
  lights were flagged. Now 31 of 87 fits decide and 1,586 lights are
  flagged: 1,036 shot during a change with nothing to place them, 550
  whose nearest flats lie across two changes. The 1,045 Question Mark
  Galaxy lights of May are placed before the donut by their own pixels.
  The same rerun leaves UltraCat131 (one change, 84 lights flagged, 23 at
  the other angle) and RedCat61 (no change) as they were.

**Response families** (the same three copies, rerun):

- **C925**: 33 families become 32, and 216 comparisons 436. The OIII
  flats at 82.2° and 72.6°, and at 79.7–82.3°, are one family
  (`72.4–72.6° & 79.7–82.3°`); 140.9° stays a family of its own, since a
  neighbour pair measured it apart from 81.2°. The three changes and their
  windows stand. Alike ranges: Hα `100.1–102.2° and 104.2–104.4°` (7
  pairs, 4 and 5 sessions), OIII `101.5–102.2° and 104.2–104.4°` (10
  pairs), and three overlapping OIII ranges between 72.4–72.6° and
  79.7–82.2°. G, R and SII have none: one end rests on a single session
  (08-24 at 104.2° for G and R, 10-03 at 101.5° for SII); B and L have no
  pair further apart than 2°. 170 lights change from main, all NGC 6543
  lights at 100.8–101.7° shot before the first change: they take the
  05-19 flats at 104.0° from their own side through a range carried
  across camera-side dust, read at 104.0° by the stacker's gate. 153 were
  flagged on main as having no flats on their side, 17 were corrected
  across the change. Flagged lights fall from 1,586 to 1,433. No direct
  pair measured apart on the same side of a change contradicts any of
  them: the only measured pairs between their flats and flats at their
  own angle cross the dust change. 48 lights were chosen by their own
  pixels, all at their recorded angle; none was moved or disputed.
- **UltraCat131**: 9 families; one range (Hα `53.4–53.5° and 163.8°`,
  measured after the 09-17 change). Every light's master, basis and flag
  is main's; the 23 Heart of the Soul R lights take the 09-16 R flats at
  3.2° by the rig's measured angle term, as on main.
- **RedCat61**: 13 families, ranges such as `180.3–181.0° and
  230.9–231.3°` in every filter (tracked sky flats behind a refractor do
  not depend on the angle), no change, no flag; every master is main's,
  since every light has flats at its own angle.
- **Rebuilds**: a stack key names every candidate flat, so a light
  whose candidates change rebuilds even when its master does not. With
  other-angle flats admitted only where nothing at the light's own angle
  can serve it, no light on the three catalogs keeps main's master and
  basis with other candidates (by the nights the reports list); on C925
  the 170 lights whose master changes rebuild, and no others.

The prototype behind these numbers, its pictures and the method are kept
with the PR's working notes; the Rust metric reproduced them.

## 8. Follow-ups

### Move to Seiza

- **Done in seiza #282**: the session response, star mask, comparison
  metrics, change map and its application, the light evidence, and the
  light-against-responses choice are `seiza_calibration::flat_response`;
  `MatchTolerances::light_rotation` lets the stacker's gate follow a
  measured angle. PSF Guard keeps the families, events, placement, the
  job, the store and the selection.
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
