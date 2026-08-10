# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- XISF frames count as images everywhere FITS frames do. Folder scans,
  imports, screening, and remote uploads now pick up `.xisf` next to `.fits`,
  `.fit`, and `.fts`, and a PixInsight-written frame grades, previews, and
  stacks like any other. A frame that says it is normalized is put on a 16-bit
  scale as it is read, so its background and star flux can be compared with
  camera frames in the same sequence instead of reading as a near-black
  outlier. See [adding image
  folders](https://github.com/theatrus/psf-guard/blob/main/docs/IMPORTING.md).
- Named processing setups. Save the parameters of the mono view-processing or
  color processing editor under a name and apply them to any card in any
  database; manage, import, and export them — singly or as one file — in
  Settings → Setups.
  Built-in setups cover the editors' defaults. See [stack
  previews](https://github.com/theatrus/psf-guard/blob/main/docs/STACKING_PREVIEWS.md#named-processing-setups).
- The Stack previews panel collapses from its title and stays collapsed
  across reloads, and its result cards follow the grid's thumbnail-size
  slider up to one full-width column.

## Changed

- Stack previews build faster. Frames are now read, calibrated, registered and
  normalized several at a time while they are still integrated in order, so the
  stack is unchanged — measured about twice as quick on local disks, and more
  when the frames sit on a network share, where the reads are what the overlap
  hides best.

- The reject archive no longer treats `.xisf` as a companion file. A frame
  carries its own grade, so archiving one alongside a rejected sibling would
  move a file the catalog still calls accepted. The default companion list is
  now `.json` and `.txt`, and a list naming any frame container — `.fits`,
  `.fit`, `.fts`, `.xisf` — is refused with an error that says why.
