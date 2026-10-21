# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- PSF Guard no longer trusts HTTPS certificates issued through emSign
  intermediate certificates that browsers have stopped trusting. This applies to
  update checks, satellite element downloads and other connections PSF Guard
  makes. It fixes RUSTSEC-2026-0382 and RUSTSEC-2026-0383.
