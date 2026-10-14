# Flat grouping

Status: **Built**. Still open: split and join by hand, moving the metric
into Seiza, and gap lights (a light whose night has no flats between two
groups that differ).
Last updated: 2026-10-14

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

All of it lives in `src/calibration/flat_groups/`. Nothing here reads a
pixel except the background job (`job.rs`).

### 2.1 Responses

A **session** is one night (at the catalog's night boundary) of one
family, with every frame's rotator angle within 2° of the first. A
**family** is one rig, filter (case-folded), frame size, binning, gain,
offset and readout-mode name; rotation clusters are formed on load.

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
library load, memoized per catalog by a digest of the rows, the rules and
the lights' times.

- **Pairs.** Within a family, every pair within twice the flat session
  window (the furthest one master can reach) and every pair of neighbours.
  Across rotation clusters of one filter, pairs shot within one window:
  these give the rig's **angle term**.
- **Groups.** Agglomerative, in time order, complete linkage: the
  neighbouring groups with the smallest largest distance merge while it is
  at most 1, where `d = max(dust / dust limit, broad / broad limit)`. A
  slow drift cannot chain nights that differ into one group.
- **Boundaries.** Between two groups: a window from the last flat before
  to the first flat after, the step's dust and broad, its spots, and a
  drift note when no single step crossed the limit.
- **Events.** Boundaries of one rig whose windows overlap and whose spots
  coincide merge. The window is the overlap. An event seen in two filters
  or at two angles is **camera side** and applies to every light of the
  rig; otherwise it applies only to its filter.

## 4. Lights

### 4.1 Placement

`Book::place(event, time)`:

1. **Time**: outside the window, the side is known.
2. **Session**: lights of one profile with no gap over 3 hours form an
   imaging session; if the session reaches outside the window, the side is
   that end's.
3. **Pixels**: the job reads the imaging sessions that lie wholly inside a
   window (`evidence.rs`). Per filter: the clipped median of up to 8
   lights, less the pedestal, divided by the nearest flat response with a
   known side, fitted against the change's map. Stars are masked by their
   cores only: a few dithered lights keep their stars, and grown as far as
   in a flat they would cover most of a star field. The fit is repeated at
   placebo positions, the template moved by whole multiples of its size;
   the weight is taken against the placebos' median, and their spread is
   1.4826 × the median absolute deviation. A side is chosen when the
   weight lies within two spreads of it and at least three from the other,
   with a spread of at most 0.3. Filters that decide must agree. Over
   smooth fields this decides; over nebulae it usually does not. The
   pedestal must come off first: left in, it damps the change and every
   light looks like it predates it.
4. **Undetermined** otherwise.

Pixel evidence is evidence about the lights; a comparison between flats is
evidence about the flats. The account names which placed the light.

### 4.2 Choosing flats

`LibrarySnapshot::choose_flats_around_changes`, after the usual gates and
nearest-first ordering, for every event that applies to the light:

1. Keep the candidates on the light's side of every event whose side is
   known. When the dropped flats could never reach the light's master
   (the nearest flat is on its side with a second within one window, and
   every dropped flat is further than two windows away), the list stays
   as it was, so the stack keeps its key.
2. None left at its angle: the nearest other angle on its side
   (`other_angle_flats`), when the rig's angle term is measured and within
   both limits.
3. Otherwise, for a camera-side event: the light's own flats, carried
   across by the change's map (`metric::change_map`), from the event's
   boundary in the light's filter or its strongest member. The built master
   is divided by the map for a light before the change and multiplied for
   one after (`corrected_flat_master`), written as
   `<master>-across-<key>.fits` with a `PSFGACRS` card, atomically.
4. Otherwise keep the other side's flats and flag (`NoSameSide`).

An undetermined light keeps the nearest flats and is flagged. The basis
travels on `CalibrationSelection::flat_basis` into the explanation
(`chosen_by`, `basis`) and the health check.

### 4.3 Validation of the order

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
  version.
- **Evidence key**: the imaging session (its profile and first light),
  the event and the evidence algorithm version. A change's key is its rig
  and window, so new flats that narrow the window ask the lights again.
- **Stacks**: `selection_hash` adds `flat-correction=<key>` for a
  corrected flat. Which flats a change leaves a light already shows in its
  candidate list.
- **Health**: `library_digest` hashes the three tables;
  `HEALTH_ALGORITHM_VERSION` is 3.
- **Stability check**: dips only; `FLAT_STABILITY_VERSION` 2, and
  judgements from version 1 are made again.
- **Pictures**: the ratio picture goes through the preview queue
  (`GenKind::FlatRatio`), keyed by both responses, the stretch and the
  render version, and reports through the shared generation-status poll.
- **Job**: a stage of the calibration quality job after grading, on the
  background worker share, yielding to interactive work, committing as it
  goes.

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

The prototype behind these numbers, its pictures and the method are kept
with the PR's working notes; the Rust metric reproduced them.

## 8. Follow-ups

- **Seiza.** The response, comparison, star mask and change map belong in
  `seiza-calibration`, so the rule "dust and dew take light away; stars
  add it" has one owner. They stay in `flat_groups::metric` until a Seiza
  release carries them.
- **Split and join by hand**, synced as choices.
- **Gap lights**: a light whose night has no flats, between groups that
  differ, takes the nearest by time without a word.
- **Pedestal refresh**: a dark-flat imported after a session was measured
  does not refresh that session's pedestal; it shifts the metrics by about
  the pedestal over the level, near 2%.
