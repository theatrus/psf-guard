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
- The Sequence score chart puts a dot under each bar for the frame's grade:
  green for accepted, red for rejected, faint grey for pending. The bar's
  tooltip starts with the grade and the reject reason, and the dots change
  as you grade, undo and redo.
- Planning no longer says **PSF Guard was updated** after every reload. A
  survey image the browser had kept from before an update carried the old
  build, and the page took it for news of a newer one.
- Changing a filter's template in a plan before its first activation no longer
  leaves the old exposure plan running in Target Scheduler beside the new one.
  Activation turns off a plan on the target that takes the same frames at
  another length, the preview lists it as replaced, and the plan asks to be
  activated until it is.
- A plan asks to be activated again whenever activation would change a rig's
  Target Scheduler rows, not only after a save: rows changed in N.I.N.A., or
  an old plan another length replaced that is still on. The plan's summary
  says **Synced** or **Not synced** (why is in the tooltip and in the bar at
  the top), or **Couldn't check the rigs** when they cannot be read, with a
  separate **Activate…** button. The bar's button says **Activate again…**
  once the plan has been activated. An exposure plan the preview updates
  says what changes, such as `desired 100 → 150`.
- The plan now decides which Target Scheduler exposure plans run in each
  rig's project. Activation switches on every plan the plan has, and
  switches off every other plan in the project, frames and counts kept,
  including filters the plan doesn't shoot at all. Two objectives that come
  to the same target, template and exposure share one Target Scheduler plan
  asking for the sum of their frames; progress shows it under each, marked
  shared, and counts it once.
- The activation preview starts with what Target Scheduler will shoot
  afterwards, then what it turns off and why, then what it turns back on,
  and shows the same after Apply. The count of turn-offs is in the rig's
  line and in the bar ("Not synced: turns off 1 plan in ultracat131").
- To keep a plan made in N.I.N.A., **Take Target Scheduler's values**: it
  now brings each exposure length in as its own objective, so a 30 s L plan
  beside the plan's 300 s one becomes an "Unsaturated stars" objective and
  stays on. Planning never adds objectives on its own: a project imported
  from Target Scheduler keeps one objective per filter.
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
  its name. Resting the pointer on any other slides its name out. The Sky
  comes first, apart from the views that follow the header's scope.
  Settings, keyboard shortcuts, server information and sign out sit in one
  menu at the right.
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
- Images, Sequence and the Calibration page share one compact toolbar: a
  row of filter pills such as **Status: All**, the thumbnail size slider and
  the tools, with display settings under **View** and rarely used tools such
  as **Move exposures** under **⋯**. A slim row below shows the counts, and
  once images are selected, **Accept**, **Reject**, **Pending** and **Stack
  selected** on Images or **Review rejection** on Sequence. On a phone the
  filters and sliders fold behind a **Filters** button that counts the
  filters in use.
- In Sequence, the **Select** pill says what each rule selects and how many
  frames, and a chosen rule stays on: moving the score cutoff or rescoring
  selects again, frames you clicked by hand stay as you left them, and the
  address keeps the rule. **×** beside the pill turns the rule off and keeps
  the frames; **Clear** drops them.
- An image's detail view has a **Calibration** panel: the light's
  calibration findings in words, and for each kind the frames its master
  would take, by night and sensor temperature, with suspect, kept-out,
  unused and refused frames and why. Choosing a finding picks out the frames
  behind it, such as the two nights a flat master mixes, and the choice
  stays as you move between images. The 🧪 chip in Images and Sequence now
  opens the light on that panel.
- Screen readers now hear which choice is in effect on more controls: the
  sequence tabs in Sequence, **Starless** and **Stars** on a stack preview,
  the start years under the Sky timeline, the plan whose workspace is open
  in the project picker, and the project you came from in the Library.
- **Use anyway** now puts a dark graded for stray light back into its
  master. Master building screens darks for stray light itself only while
  some dark in the set has no grade yet, and a master's record lists only
  the frames it combined. After the upgrade, each dark and dark-flat master
  whose frames are all graded builds once more, and so do the flats and
  stacks built on it. A newly imported dark set builds twice: screened
  before grading reaches it, then without the screen once it is graded.
- `--allow-database-management` now guards only what lets a client name a
  server path: registering or changing databases and their folders, storage
  folders and limits, the PixInsight program and runs folder, a WBPP run
  folder outside that runs folder, imports from outside the image folders,
  local exports to a chosen path, and the new-frame and calibrated-copy
  switches. An editor no longer needs it to keep calibration frames out or
  use them anyway, mark their validity, forget them, clear masters,
  invalidate flat coverage, move targets and exposures, set exposure
  grouping or the AstroBin filter map, fill in GUIDs, run WBPP, install Seiza
  catalogs, manage and sync with peers, pair clients, or move rejects to the
  trash and back. Viewers get none of these. Only an editor reads a WBPP
  run's files, a link in a run folder can no longer lead outside it, and a
  calibration frame recorded outside the database's folders shows as
  missing instead of being read.
- Calibration health catches up on its own. A light whose file was missing
  is judged again once a directory refresh finds it or the image folders
  change. New lights from N.I.N.A. or a CLI regrade are judged once the
  Library or an image view looks, within a couple of minutes and after any
  grading pass. Judging a project again reads only the lights whose file
  changed, and a server that stops ends the work between lights. The first
  pass after the upgrade judges every project and reads every light's
  header once.
- Sequence holds back **Poor calibration** in **Select**, marked
  *(updating)*, until the calibration check has caught up with recent
  changes.
- A night's "can't be calibrated" link in the coverage report opens only
  that night's lights, and Images keeps that night when you change another
  filter.
- On the Calibration page, stepping through flats in compare mode stays in
  the comparison instead of flashing back to the plain frame and losing the
  zoom. Changing database clears the last database's rig and filters, and
  each project opens on its own rig unless you chose one by hand.
- A calibration session's address no longer changes when a colder or
  warmer frame joins the session. Every dark and flat session gets a new
  address once with this version, so a link saved before it opens the first
  session instead.
- Import attaches lights to an exposure plan of their length whose template
  has their filter and binning, with gain, offset and readout mode equal to
  theirs or left at the camera's default (-1). Before, it held such lights
  as having no exposure plan, or added a second template and plan. A new
  plan goes on a template of the lights' filter, binning and length, again
  allowing -1 for the rest, and only without one does import make a
  template pinned to the lights' gain and offset. Groups that land on one
  template share one plan.
- The transfer preview counts the **Keep out** and **Use anyway** choices a
  pull brings, since they change the destination's masters, and a choice
  made on the destination stays with a file the source cataloged under
  another ID.
- Scan rules also hold when an import starts inside an excluded or ignored
  folder, as **Review** does. A full path saved as a pattern is refused with
  the pattern to write instead, and automatic import no longer rereads
  lights its filters left out while their files are unchanged.
- Restarting the server from the desktop app's Settings waits for the old
  server to finish its work, so the job queue it saves is not lost. If that
  work is still running after two minutes, the log lists it and the whole
  app restarts instead.
- A plan whose remote rig's site missed the last activation now reads **Not
  synced**: "observatory-pi hasn't received this activation" in the summary
  and the bar, and "not sent to observatory-pi" on the rig's line. The bar
  and the activation dialog offer **Send to observatory-pi again**, with the
  error and when it was tried, and show no button when every site has it.
  Once the site answers a connection test, a sync or another push, the
  server sends the activation there on its own.
