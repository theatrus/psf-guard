# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- Export can lay a session out for PixInsight's WeightedBatchPreprocessing.
  Pick **WBPP** as the export layout on the overview, or pass `--layout wbpp`,
  and each frame type gets its own folder with dark flats among the darks
  where WBPP expects them. The export carries `run-wbpp.sh` and `run-wbpp.cmd`
  that hand it straight to PixInsight, stopping at WBPP's dialog so you can
  check the groups before starting. The existing target-grouped layout stays
  the default. See [calibration
  libraries](https://github.com/theatrus/psf-guard/blob/main/docs/CALIBRATION_LIBRARY.md#export).

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

## Fixed

- Imported frames now show star count and HFR after quality analysis runs.
  Header-first imports carry no star metadata, and the quality scan kept its
  measurements in a cache the grid, detail, and comparison views never read —
  so a catalog built from image folders (FITS or XISF) looked unanalyzed no
  matter what ran. The scan now writes its measured star count and HFR into
  each imported image's metadata, filling only what is missing; values a
  N.I.N.A. catalog already carries are never touched. Catalogs analyzed
  before this fix pick the values up the next time **Analyze Missing
  Quality** runs, straight from the cache. A checkbox beside the analyze
  buttons turns the write-back off, and the choice is remembered as the
  default for every analyze action. See
  [importing](https://github.com/theatrus/psf-guard/blob/main/docs/IMPORTING.md).

- A stack whose reference frame sits on the thin side of a meridian flip is
  turned the right way up again. The reference's exposure was weighed in
  seconds while every other frame counted as one, so on a catalog that records
  no exposure length the reference alone outvoted the whole night and the stack
  published upside down.

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
