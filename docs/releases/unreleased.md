# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- PSF Guard now reads a N.I.N.A. horizon file that lists one azimuth on
  several lines, as a file may do to draw the upright edge of a building, in
  **Rig setup** and **Settings → Presets → Sites**. Before, it refused the
  file. That azimuth takes the highest of its altitudes, so a target must
  clear every line.

- Horizon files are now read as N.I.N.A. reads them in two more ways:
  commas, semicolons and tabs separate the columns, and a file with neither
  0° nor 360° takes the altitude of the point nearest north at both ends.
  Before, PSF Guard drew a line across north between the last and first
  points, so its horizon near north could differ from N.I.N.A.'s by a few
  degrees. A horizon already saved keeps its curve until you paste or
  upload the file again.

- **Run now** no longer fails with "an import is already running for this
  database" while PSF Guard receives a remote upload or finishes other
  work on the catalog. The run waits for that work and then starts.

- When one Target Scheduler database serves several N.I.N.A. profiles, a
  rig now offers only its own profile's exposure templates. Before, the plan
  editor could give a plan another profile's template of the same name, with
  that profile's gain and exposure, and **Copy from** on **Settings → Presets
  → Exposure templates** listed each name once per profile. **Copy from** now
  says how many templates from other profiles it leaves out. A template whose
  name is in the library with other settings reads **differs from the
  library**; click it to replace the library's settings with the rig's.

- **Folders & limits** heads each disk with the folder that holds its storage
  folders, such as `./cache`. Before, it showed one database's folder inside
  the cache, such as `./cache/db-e075b4ec`, though the disk's use and limit
  cover every database on it.

- Release downloads omit duplicate installers and the extra macOS app ZIP.
  Windows MSI installers remain available. Update notices keep dotted names
  such as N.I.N.A. within the first sentence.
