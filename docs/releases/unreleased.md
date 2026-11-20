# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- A N.I.N.A. horizon file that gives one azimuth on more than one line, as
  files often do to draw the upright edge of a building, is now accepted in
  Rig setup and Planning sites. Before, PSF Guard refused it. The azimuth
  takes the altitude of its last line, as N.I.N.A. reads it, so Planning
  plans against the same horizon N.I.N.A. uses.

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
