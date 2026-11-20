# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- **Folders & limits** heads each disk with the folder that holds its storage
  folders, such as `./cache`. Before, it showed one database's folder inside
  the cache, such as `./cache/db-e075b4ec`, though the disk's use and limit
  cover every database on it.

- Release downloads omit duplicate installers and the extra macOS app ZIP.
  Windows MSI installers remain available. Update notices keep dotted names
  such as N.I.N.A. within the first sentence.
