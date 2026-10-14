# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

- A **Calibration** page shows a database's calibration library: its sessions
  by night, kind and settings, each frame as a picture drawn for its kind,
  and a flat divided by the rest of its session so that drying dew stands
  out from dust. Beside each frame are its grade and its numbers next to its
  session's, with **Keep out**, **Use anyway**, validity marks and
  **Forget**. ← → flip through a session. A **Masters** tab lists every
  master with its sources and the projects using it, and opens any of them
  without a stack. Open it from Settings › Databases or a project's
  Calibration report; it replaces the calibration library dialog, and its
  filters and choices stay in the address.
- The Library flags projects whose lights the calibration library serves
  poorly with a **Poor** or **Check** pill, worked out in the background:
  lights it cannot calibrate (now also lights from a camera with no
  calibration frames at all), thin dark masters, flats that carry changing
  spots, old or mixed flats, suspect frames in use. The pill opens the
  Calibration report, whose new **Quality** section counts the lights by
  night and filter, and **Show › Calibration issues** lists just those
  projects.
- PSF Guard grades every bias, dark, dark-flat and flat in the calibration
  library as OK, suspect or poor. Flats are judged against the rest of their
  session for clipping, exposure, the light pattern and spots that come and
  go, such as drying dew; bias and darks against the frames that could stand
  in for them for level, noise, banding and hot pixels. Poor frames stay out
  of masters and WBPP exports, and stacks that used them are built again. A
  dark that caught stray light is one kind of poor frame.
- The calibration library shows each frame's grade and its main reason, and
  each night counts the frames kept out. **Keep out** leaves a frame out of
  masters and exports without forgetting it, so a rescan does not bring it
  back; **Use anyway** puts a poor frame back; **Clear** lets the grade
  decide again. Choices travel with the frames when catalogs sync.
- Grading runs in the background when the server starts, after each import,
  and two minutes after remote uploads of any calibration frame stop, and
  gives way to work you are waiting for.
- PSF Guard 0.11 keeps working on a catalog this version has graded. It
  leaves out the darks graded for stray light and ignores the other grades
  and your choices.
- PSF Guard checks darks and dark-flats sent by remote upload for stray light
  once uploads pause for two minutes, as it does after an import; before,
  they waited for the next server start. The calibration library shows each
  measured dark's level and spread, in ADU.
- Every stack and master rebuilds once, the first time each is used, after
  the move to Seiza 0.29.
- Automatic import no longer creates projects on its own. Frames for targets
  the catalog does not have wait, and the database's status line offers
  **Review** to preview and import them; **New targets** in its settings
  can leave them out or create projects as before. Scan rules leave
  autofocus frames, tests and other folders out of every import, with exclude
  and include patterns and filters for frames with no image type, no target
  name or a short exposure. A `.psfguardignore` file skips its folder.
- Planning no longer says **PSF Guard was updated** after every reload. A
  survey image the browser had kept from before an update carried the old
  build, and the page took it for news of a newer one.
- Changing a filter's template in a plan before its first activation no longer
  leaves the old exposure plan running in Target Scheduler beside the new one.
  Activation turns off a plan on the target that takes the same frames at
  another length, the preview lists it as replaced, and the plan asks to be
  activated until it is.
- Automatic import no longer changes what Target Scheduler will shoot in
  projects it did not create. There it only adds image rows to exposure
  plans that already exist, and leaves their counts alone. Frames that would
  need a new plan wait for **Review** with the rest. The import preview now
  says which plans an import adds or raises.
- A running stack's status, which the page asks for every second, no longer
  carries each frame's background map. For a 122-frame stack that cut each
  reply from 1.1 MB to about 70 KB.
- A project's calibration coverage report leaves rejected lights out, so a
  night you rejected no longer shows up as lacking flats or darks.
  **Include N rejected lights** adds them back.
- Images, Sequence and a mosaic's sequence mark lights the calibration
  library serves poorly with a 🧪 chip: red for Poor, yellow for Check. The
  chip has its own toggle. Images gains a **Calibration** filter, and
  Sequence a **Poor calibration** choice under **Select**. In a project's
  Calibration report, each finding and each night that can't be calibrated
  opens those lights in Images.
- The Library uses the whole width of the window, up to 2400 px. In the
  compact view each project stays on one line on a laptop or wider screen,
  and a long name ends in "…" rather than pushing pills onto a second line.
  On a phone, a project row that wraps is a rounded box, not an oval.
- The Calibration page has a tab in the header, between Sequence and
  Stacks. It follows the database in the header's scope rather than keeping
  its own, and with a project in scope it opens on that project's rig. A
  stack's master inspector opens a master on the page with **Open in
  Calibration**.
- The header is narrower. Its views show as icons; the current one keeps
  its name. Hovering any other slides its name out, and the current one
  folds to its icon meanwhile, still highlighted. The Sky comes first,
  apart from the views that follow the header's scope. Settings, keyboard
  shortcuts, server information and sign out sit in one menu at the right.
  The **Editor** and **Read only** label is gone; the menu says who is
  signed in.
- Saving a project or adding an exposure plan no longer fails with "database
  is locked" when a background pass or N.I.N.A. is writing to the catalog at
  the same moment. The save now waits for that write to finish. Imports,
  syncs, forgetting calibration frames, and moving or restoring rejects wait
  the same way.
- On the Calibration page, **Keep out** is offered for every frame, even one
  its grade already keeps out, so you can keep a frame out by hand the way
  you reject a light, and a later grade cannot bring it back. Such a frame
  shows **Kept out by hand**.
- Stopping or restarting the server no longer hangs until systemd kills it.
  Background work stops at its next safe point: a quality scan saves what it
  measured, a stack build checkpoints the frames it integrated and resumes
  after the restart, and imports and calibration grading stop at a commit.
  The log says what the stop is still waiting for. A stack build stopped this
  way no longer counts toward the two restarts that drop a build.
- Images, Sequence and the Calibration page share one toolbar: filters in a
  row of labelled fields, Status and the other filters as one kind of
  multi-select, and the same buttons everywhere. On a phone the filters fold
  behind a **Filters** button that counts the ones in use. Selecting images
  turns the toolbar's last row into a bar with the count and the actions:
  **Accept**, **Reject**, **Pending** and **Stack selected** on Images,
  **Review rejection** on Sequence.
- In Sequence, **Select frames** says what each rule selects and how many
  frames, and a chosen rule stays on: moving the score cutoff or rescoring
  selects again, frames you clicked by hand stay as you left them, and the
  address keeps the rule. **Stop rule** keeps the frames; **Clear** drops
  them.
