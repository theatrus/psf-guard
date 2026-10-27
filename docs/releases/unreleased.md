# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- PSF Guard's zstd library is updated for memory-safety advisories in its
  stream and context handling. PSF Guard uses zstd to read and write
  compressed XISF files and saved stacking contexts, and to decompress HTTP
  responses. It fixes RUSTSEC-2026-0391, RUSTSEC-2026-0398, RUSTSEC-2026-0403
  and RUSTSEC-2026-0410.
