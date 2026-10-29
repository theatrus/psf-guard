# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- PSF Guard starts when one of a database's image folders is missing, for
  example after you move the folder or before its drive is mounted. Before,
  the server stopped with "Image directory not found" until the folder was
  removed from the configuration by hand. Frames in a missing folder show as
  not found until the folder is back, and you can change the database's
  folders in **Settings**. Imports skip a missing folder and import the
  others. Remote image uploads to a missing folder are refused, and the
  upload setting stays on.

- PSF Guard's zstd library is updated for memory-safety advisories in its
  stream and context handling. PSF Guard uses zstd to read and write
  compressed XISF files and saved stacking contexts, and to decompress HTTP
  responses. It fixes RUSTSEC-2026-0391, RUSTSEC-2026-0398, RUSTSEC-2026-0403
  and RUSTSEC-2026-0410.
