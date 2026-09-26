# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- The server is an **MCP server** at `/api/mcp`, so an agent such as
  Claude Code can list catalogs, read grades and quality evidence, score
  sequences, apply grades, and start imports, quality scans and WBPP runs.
  Write tools follow the caller's role, and the WBPP tools follow the
  database-management gate as the UI does. See `docs/MCP.md`.
- **API tokens** for scripts and MCP clients: mint one under **Settings →
  Users → API tokens** or with `psf-guard users token create`, send it as
  `Authorization: Bearer psfg_…`. A token acts as its user, can be made
  read only or given an expiry, and is shown once. Revoke it in the same
  place or with `users token revoke`; removing a user revokes their tokens.
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
