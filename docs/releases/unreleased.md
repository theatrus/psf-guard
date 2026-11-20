# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- After its first build, preparing a stack build no longer opens every light
  and calibration file. The catalog keeps each light's header, where the
  last build found it, and which calibration files it checked, so PSF Guard
  reads a file again only after the file or its catalog row changes. On a
  1,263-light project on a network share, preparing took about 1.3 seconds
  where it took about 2.3, and the calibration health pass, the
  uncalibrated-lights check and Calibration's project scope share the same
  kept headers.

- Release downloads omit duplicate installers and the extra macOS app ZIP.
  Windows MSI installers remain available. Update notices keep dotted names
  such as N.I.N.A. within the first sentence.
