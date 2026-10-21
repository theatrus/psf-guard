# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- Previews no longer fail when a frame is in two image folders, such as
  while it is copied from an incoming folder to its final folder. PSF Guard
  reads the largest copy of the frame, so a copy that is still being written
  is not used. Files of the same name that hold different captures still
  show an error. See
  [Frames in more than one folder](../IMPORTING.md#frames-in-more-than-one-folder).
