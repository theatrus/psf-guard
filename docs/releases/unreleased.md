# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- WBPP runs **queue**. PixInsight runs one job at a time across all
  databases; starting a second project while one runs puts it in line,
  shown on the Overview with its place and a **×** to take it out, and it
  starts on its own when the running job ends. **Queue stacking** replaces
  the refusal.

- **Automatic import.** A database can scan its image directories on its
  own, on open and on a schedule, and import the lights and calibration
  frames it does not have yet. Turn it on per database under **Settings →
  Databases → Edit → Automatic import**, choose the frame kinds and
  whether to analyze quality afterwards. A run drops every file the
  catalog already holds before reading a header, so a folder of known
  frames costs next to nothing. The database row shows the schedule, the
  last run, and a **Run now** button.

- The Overview shows every database's WBPP run under way or just
  finished in any browser tab, as a status line and as the state of the
  project's **Stack in WBPP** action, and either reopens the run's dialog.

- **AstroBin** action on every project card and target row of the
  Overview, and an `astrobin-csv` command, write the acquisition CSV
  AstroBin's upload page imports: one row per night, filter and exposure
  length. **Essentials** is the date, filter, count and duration; **Full**
  adds binning, gain, sensor and ambient temperature, the f-number, and
  the darks, flats and bias the calibration library matches to each night.
  The dialog previews the rows and asks for the AstroBin id of any filter
  it does not know yet.
- Each catalog keeps its own **AstroBin filter map** (**Settings →
  Databases**): which AstroBin filter each name stands for on that rig,
  with a first and last night for a filter that changed over time. The
  export reads it ahead of the server-wide defaults under **Setups**.

- **Stack in WBPP** on a project card runs PixInsight's
  WeightedBatchPreprocessing on the server, against the frames where they
  are, headless (through `xvfb-run` on a server with no display). The
  dialog follows WBPP's own log and lists the masters as downloads when
  it finishes. **Settings → Setups → PixInsight** names the install and
  the **runs folder**; without one, runs go under the database's export
  directory, else the cache, and the free space there is shown. A
  database's new **process directory** (**Settings → Databases**) lets a
  run save its masters beside your finished work, in a per-project folder
  PSF Guard remembers, without overwriting anything already there.
- **WBPP settings** for exports and runs: quality (WBPP's presets), Fast
  Integration, drizzle (2x or 3x), autocrop, and the light rejection
  algorithm, with defaults under **Settings → Setups → Export** and
  matching `--wbpp-*` flags on `psf-guard export`. Fast Integration is off
  by default: WBPP would otherwise switch any group of 150 frames or more
  to it on its own during a headless run.

## Changed

- A WBPP export's launchers now always start `run-wbpp.js`, which carries
  the frames or folders, every setting, and the per-group drizzle and Fast
  Integration choices WBPP has no command-line parameter for. Anything
  appended to `PARAMS` still reaches WBPP.

## Fixed

- Opening **Stack in WBPP** on one project while another project's run
  was under way titled the dialog and its progress after the project you
  clicked. The dialog now opens under the clicked project, names the run
  PixInsight is busy with, and offers to queue; **Show that run** switches
  to the running one.
- The Overview's WBPP and export status lines share one style, coloured by
  state, and a finished WBPP run can be cleared with **×**.

- Flat coverage sync from the N.I.N.A. plugin no longer fails with
  "flat-history fingerprint was reused for different capture metadata"
  after a server update. Rotation and ROI now compare within a tolerance,
  so a change in how the server parses a 17-digit float no longer reads as
  a changed capture; the first replay rewrites the stored values.
- An RGB master, such as one WBPP integrated, now previews in colour with a
  stretch per channel. Before, its preview crashed the preview worker.
