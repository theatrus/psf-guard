# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

- Upgrading PSF Guard no longer stacks every channel again and rebuilds every
  calibration master when the new Seiza release leaves stacking output
  unchanged. **Build current set** reuses the existing stacks.
- A Seiza release that changes only how previews are stretched no longer
  stacks anything again. **Build current set** renders each card preview
  again from the stack's FITS. A one-shot-color stack whose color calibration
  is out of date renders again the same way; it used to need a rebuild.
