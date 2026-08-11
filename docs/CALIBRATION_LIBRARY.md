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
field as a gate. A match still needs a positive camera-name or sensor-size
identity; wholly unknown sensors never match. It sorts safe candidates by
distance from the light's capture time and uses at most 64 frames per master.

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
on the export download, or the **Export layout** control on the overview, which
applies to every export on the page.

Rejected lights remain excluded in either layout. Repeated calibration sources
are deduplicated per destination. Name collisions receive a numbered suffix.

## Database transfer

**Merge catalogs** and `sync pull` copy rigs, profile bindings, and raw
calibration-frame records. They do not copy generated master rows or cached
FITS files because those paths belong to the source machine. The destination
resolves a stale raw path by basename in its configured image folders, then
builds its own master.

As with other transfer work, the UI requires a dry preview before apply.
