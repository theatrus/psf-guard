# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- On a Mac, PSF Guard saves to a network share (SMB), such as a NAS:
  calibration masters, stacks, remote uploads into an image folder, WBPP
  work files, backups, and settings kept on a share. Before, those saves
  failed with "Operation not supported (os error 45)", because the share
  refuses the full sync macOS asks for. PSF Guard now falls back to a plain
  sync there, through Seiza 0.48.2.
- **Rigs** (early beta), a new view beside **Sky**, holds every rig in one
  place. Its **Live** tab is the Live table, which used to sit under the
  Sky's map. **Rigs** in the header opens it and shows the fleet's status,
  which the separate rigs chip used to show. The Sky's **Live** still draws
  each rig where it points. Its **Nights** tab shows every rig across the
  coming nights: the clear dark hours, the Moon, and the plans each rig
  would shoot. Choose a night to see its schedule, a prediction of what
  Target Scheduler will shoot there and why it picks each target, or what
  Director would do; tonight is planned from now. The night lists only the
  targets it applies to, and folds the rest with the reason each is out.
  Mark a night available, not available, or clear only for some hours, for
  one rig or for every rig at a site; the schedules work around the marks.
  The Live table's new **Tonight** column links each rig to tonight's
  schedule. See [Nights](../DIRECTOR.md#nights).
- PSF Guard imports a calibration master built by `seiza master` as a
  calibration master. Seiza 0.48.1 and earlier write a master's kind only in
  `SEIZAMST`, and earlier PSF Guard versions imported such a master as a
  light under "Unknown Target". Like other external masters, a Seiza master
  is used in place of one PSF Guard would build from raw frames, unless
  **External masters** in **Settings › Processing › Calibration › Matching**
  says otherwise.

  To fix masters that an earlier version imported as lights:

  1. In **Settings**, select the database, open **Import**, click
     **Preview import**, then click **Import N frame(s)**. PSF Guard adds the
     masters to the calibration library. The automatic import does not read
     these files again, so this manual import is needed.
  2. In the Library, open each Unknown Target project that holds only
     masters, open **Edit project**, and click **Delete project...**. PSF
     Guard removes the light rows and keeps the files on disk. Deleting a
     project needs an editor's account and a server started with
     `--allow-database-management`.
- An import leaves stacks out, and the import result shows how many. Before,
  a stack, such as a file with `NCOMBINE = 40`, was imported as a light. A
  stack already in your catalog stays there. Remote upload refuses a stack.
- The **Include only** scan rule shows "All files" when it is empty. Before,
  it showed a grey "LIGHT" that looked like a value.
