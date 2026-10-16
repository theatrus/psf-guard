# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

- The header's background jobs keep automatic work in view while your own
  work runs. A database quality analysis, an automatic stack refresh, or a
  re-apply of processing that steps aside for a build, scan, or solve you
  started is listed as **waiting**, with what it waits for and how far it
  had got, below your work. The chip counts running and waiting jobs with
  a play and an hourglass icon each, so it stays as small as before; a
  phone shows the waiting count only. Builds you queued count as queued,
  not waiting. An automatic build your build stopped starts again once
  your work has been quiet for 30 seconds, from where it stopped when no
  frame or grade changed meanwhile, and its row turns into the running
  build without appearing twice. One you stopped yourself stays stopped.

- Collaboration imports preserve Starfront's physical-filter/data-band mapping
  and OSC camera and Bortle limits in reviewed plans. Reports carry the data
  bands explicitly, and work for bands disabled by the project stays held.
- Collaboration **Reports > Contribution files > Upload pending** sends raw accepted lights
  with the same matched calibration inputs as exports, or an existing PSF Guard
  or WBPP stack. Matching and validation run in the background; only exceptions
  require review. Upload manually, or opt into automatic raw/PSF Guard
  stack delivery for the rig's database. **Transfers** in background activity
  shows progress, Pause, Resume, Retry and Cancel. Completed-file receipts
  survive restart; interrupted files retry without duplicating server files.
  Delivery receipts are shared across jobs by destination and file content revision,
  so shared calibration files are not repeatedly uploaded. Automatic raw delivery
  queues new exposures and changed source revisions. Retrying an exception checks
  the current remote owner's receipts, and automation settings wait for their
  saved policy before allowing edits.
- Flat measuring is a job of its own in the header's job line, **Measuring
  flats**, with the rig, `12 of 40 sessions` and a Stop, and Calibration ›
  Frames shows the same line while it measures a rig in view. It measures
  only the rigs and sessions an import, sync, upload, forget or duplicate
  repair touched, once a burst of them pauses, and at start picks up only
  unfinished work, so an up-to-date catalog reads no pixel. Changing the
  dust or light-pattern limit regroups at once without grading the library
  again; switching a rig from grouping by time to by measurement measures
  that rig alone, and a rig grouped by time is never measured. While Frames
  shows one rig's flats, that rig is measured first, without holding up a
  stack, a preview or anything else you start. Work it could not finish,
  such as flats on a share that is away, is tried again later, and Stop
  pauses measuring until those flats change, **Resume now**, or a restart.

- Images' **Date** groups are observing nights, split where the catalog's
  nights split, as calibration and the AstroBin export do: a night west of
  Greenwich no longer lands on the next day's date or across two dates.
  Where one target and filter hold frames from two or more Target Scheduler
  profiles, sequence scoring and statistical grading compare each profile
  with itself, so two rigs merged into one catalog are no longer scored
  against each other. The profile stands for the rig because catalog rows do
  not record the camera or telescope, and reading every frame's header would
  make scoring slow, so one rig kept under two N.I.N.A. profiles now scores
  as two. A catalog with one profile scores as before, including older rows
  that name no profile. Two projects with one name stay in separate Project
  groups. Links to groups from before still open them, and the image
  detail's next and previous walk the same nights as the grid.
- A Director plan's status now reads **Activated** or **Needs activating**
  instead of Synced or Not synced, so it is not mistaken for the catalog
  Sync, and the Library badge says **Activated on N rigs** rather than
  Active, Target Scheduler's own project state. The **Activate on the
  rigs** dialog previews by itself when it opens and again whenever its
  inputs change, so there is no Preview button to press. The preview reads
  a copy of each rig's planning tables, so it never waits for N.I.N.A. or
  holds a rig database's lock; **Activate** checks the databases themselves
  and writes only when they match, and when they do not, it says what
  changed and previews again. A plan that cannot be activated yet says
  what to do first instead of offering **Activate…**. Unsaved plan edits
  are never saved unasked: the dialog offers **Save and activate…**.
- **Calibration** opened from a project shows the calibration that project
  uses: the sessions its lights' masters are built from, flat grouping's
  choices included, the masters they calibrate with, and only the flat
  groups they touch, across every rig the project was shot with. An
  **M 44 on Askar 120 ✕** chip heads the filter bar; ✕ or **Show all of
  Askar 120's calibration** widens to the rig's whole library. Opened from
  the Library, it shows the whole library as before. On Calibration the
  header's Rig · target control names the rig only.
- **Calibration › Frames** can list flat groups instead of sessions:
  **Sessions | Groups** at the top of the sidebar. Groups run newest first
  by filter and angle, with the split between neighbours and its reason
  in between, and mark the current group, one partly measured and one a
  change follows; ↑ and ↓ move between them. A group shows its flats in
  the same image browser as a session, night by night with each night
  marked, then its nights, how alike they measured against the rig's
  limits, the change on each side with Compare, and what it feeds: its
  flat masters and the lights that take it, by project, each linking to
  Images on exactly those lights. The group lives in the address, and
  scoped to a project the list holds that project's groups with their
  neighbours dimmed.

- **Settings › Server** can fix the desktop app's localhost port, so browser,
  API, and agent connections keep the same address across restarts. A port
  conflict shows the error and Server settings so you can choose another
  port or return to Automatic.

- A calibration frame that is on disk twice, such as a copy a filer has not
  yet cleaned out of the drop folder, is catalogued once. When the original
  goes, its row moves to the copy and keeps its grade and marks, instead of
  staying behind as a missing file. The Calibration page's **Duplicates**
  filter lists exposures an older import recorded twice, and **Repair
  duplicates…** previews and then merges them into one row each and rebuilds
  the masters built from them. A later pull from a catalog that still has
  the duplicates does not bring them back. No file is moved or deleted.
  `psf-guard calibration repair-duplicates <db> --dry-run` does the same from
  the command line.
- The thumbnail size slider in Images and Sequence counts every step of a
  quick run of arrow keys. On a busy page it used to snap back between
  steps, so some were lost.
- With reduced motion turned on in the system, loading spinners hold still.
- The reload button left the header. **Rescan image folders**, under ⋯ in
  Images and Sequence, looks for image files added since the last scan
  (folder tree and files together; no Shift-click). PSF Guard still rescans
  a database's folders on its own every five minutes and after sync and
  automatic import.
- While a plan's framing has a locked shot panel, every rig that is on and
  did not shoot it shows its field as an outline where the panel was shot,
  so turning a rig on or picking it shows what it would take there.
  Unticking **Outline** hides it for the visit.
- A plan's mosaic panels are placed one by one. A framed target that already
  has frames starts locked where they were shot, and **Unlock** frees it;
  **Mosaic panels** adds
  panels beside it or round the whole mosaic to the north, east, south, west
  or a corner, as a ring, or by filling holes. With **Panel** you drag one
  panel on the sky: it settles at the minimum overlap, shows what it shares
  with its neighbours, and a drop that would leave a gap or too thin an
  overlap goes back unless you hold Alt or press **Place anyway**. With
  **Snap** off, a dragged panel stays where you drop it, flagged if it
  breaks the overlap rule. An unlocked panel with frames that is moved far or
  turned gets a new Target Scheduler target; the old one keeps its frames. Arrow
  keys nudge the chosen panel, and panel edits undo. Activation adds a Target Scheduler target per new
  panel in the same project and leaves the shot one alone, and the Library
  shows the panels as one mosaic whatever their shape. Saved grids open
  unchanged, as the panels they made.
- A shot mosaic panel is drawn where its frames were taken, with the field
  and angle they were shot with, read from their plate solves, else their
  headers, else the plan. The sky shows a panel's name, small, only when
  the mosaic has more than one panel. New panels use the rig's
  current field and meet a shot panel at its real edges. Each panel is now
  shot at its own camera angle, the plan's turned by north's lean at its
  center, so wide northern mosaics stay square; targets already shot keep
  their angle. A panel can also be turned on its own with its handle, `[`
  and `]`, or a typed turn. Every rig gets each panel's angle, rotator or
  not. A rig framed on its own gets the same panel tools.
- Picking another rig for a shared mosaic of more than one panel no longer
  moves its panels to that rig's field: the rig's card offers **Frame
  separately** or **Use its field**, and a mosaic keeps its size when its
  rig goes off. A rig framed on its own is checked with the other framings
  on the sky, so its panels need not touch where another rig's cover the sky
  between them; gaps, lone panels and groups apart are said for the whole
  object. A seam between two rigs' panels needs the larger of their minimum
  overlaps, counted on the smaller panel, or it is flagged as thin.
- Activation no longer takes over an existing Target Scheduler target with
  frames that sits more than a quarter of a panel from the panel it would
  stand for: the panel gets a new target, the frames stay where they were
  shot, and the preview says how far away the old target is.

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
  the move to Seiza 0.31 and stacking 0.34. Astrometry, satellite, stretch,
  and background cache metadata now follows the installed engine versions,
  so later dependency updates also invalidate affected cached results.
- Automatic import no longer creates projects on its own. Frames for targets
  the catalog does not have wait, and the database's status line offers
  **Review** to preview and import them; **New targets** in its settings
  can leave them out or create projects as before. Scan rules leave
  autofocus frames, tests and other folders out of every import, with exclude
  and include patterns and filters for frames with no image type, no target
  name or a short exposure. A `.psfguardignore` file skips its folder.
- PSF Guard measures how alike each night's flats are and keeps nights
  that differ, such as before and after new dust, out of one master. A
  light takes flats from its own side of each measured change: its own
  angle's when there are some, else the nearest other angle when the rig's
  flats barely depend on the angle, else its own flats corrected across
  the change. The light's Calibration panel says how its flats were
  chosen, and **Flats from across a change** flags a light that had to
  take flats from the other side, or was shot during the change. Nights
  measured alike now mix without the **mixed flats** Check. The
  Calibration page's **Frames** view marks each flat session with its
  group, draws each split and each camera-side change between the
  sessions with why it happened, and compares the two nights' flats. Its
  **Edit limits** opens the three settings under **Settings ›
  Calibration › Flat groups**, which set the limits or turn it off. A new
  **Angle** filter narrows Frames to one rotator angle. `psf-guard flat-groups` measures a copy of a catalog
  from the command line. Exports and WBPP runs place the same flats, give
  lights of one night that need different flats a session each, and name
  the lights WBPP cannot calibrate as a stack would.
- Flat groups hold up on rigs whose rotator wanders: flats a degree or two
  apart are one angle and are compared, so a change's window ends at the
  first flats that show it. Flats that record no angle take the angle
  they measure alike with when only one fits, and otherwise serve a light
  only when no flat of its angle does, or when they are the only flats on
  its side of a change; lights on such rigs may take new flats, and their
  masters rebuild. Flats shot through a reducer are never compared with
  flats shot without it. A light's own pixels now place it against a
  change of several shadows spread across the frame, so far fewer lights
  shot during a change are flagged. One filter's split across two changes
  no longer merges them into a window of minutes or makes a change of its
  own, a change narrower than one flat run says so in Frames, and flats
  from across two changes are flagged rather than corrected across one.
- Tracked sky flats with stars no longer read as dew or frost: the flat-set
  check counts only spots that take light away, and **Compare with
  session** draws stars at neutral grey.
- The Sequence score chart puts a dot under each bar for the frame's grade:
  green for accepted, red for rejected, faint grey for pending. The bar's
  tooltip starts with the grade and the reject reason, and the dots change
  as you grade, undo and redo.
- Planning no longer says **PSF Guard was updated** after every reload. A
  survey image the browser had kept from before an update carried the old
  build, and the page took it for news of a newer one.
- A scheduling save that conflicts with another edit blocks another save
  until you reload the limits, including while the latest limits are loading.
- Changing a filter's template in a plan before its first activation no longer
  leaves the old exposure plan running in Target Scheduler beside the new one.
  Activation turns off a plan on the target that takes the same frames at
  another length, the preview lists it as replaced, and the plan asks to be
  activated until it is.
- A plan asks to be activated again whenever activation would change a rig's
  Target Scheduler rows, not only after a save: rows changed in N.I.N.A., or
  an old plan another length replaced that is still on. The plan's summary
  says **Activated** or **Needs activating** (why is in the tooltip and in the bar at
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
  and shows the same after **Activate**. The count of turn-offs is in the rig's
  line and in the bar ("Needs activating: turns off 1 plan in ultracat131").
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
  template pinned to the lights' gain and offset, with their length as its
  default. A template whose default is another length never takes a new
  plan, so lights of two lengths get two templates. Groups that land on one
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
- A plan whose remote rig's site missed the last activation now reads
  **Needs activating**: "observatory-pi hasn't received this activation" in the summary
  and the bar, and "not sent to observatory-pi" on the rig's line. The bar
  and the activation dialog offer **Send to observatory-pi again**, with the
  error and when it was tried, and show no button when every site has it.
  Once the site answers a connection test, a sync or another push, the
  server sends the activation there on its own.
- The Library's compact rows line up in columns: database, state, progress,
  grade counts, dates, calibration, **Sequence** and **Stacks**, and ⚙ each
  sit at the same place on every row, and a project without a pill leaves
  its space empty. A long name ends in "…" and shows in full on hover. On a
  laptop the dates show how long ago the last frame was and keep the range
  in the tooltip; on a tablet a row takes two or three lines, and on a phone
  it stacks one pill kind per place. The Projects toolbar uses the pills
  Images and Sequence use and wraps into rows instead of running off the
  edge, so **Compact** / **Detailed** stays in reach on a narrow window.
- The panels beside an image in its detail view fold away: click a panel's
  title to fold or open it. A folded panel keeps one line, such as the
  quality score, whether the field solved, or the calibration findings by
  name, and each stays as you left it from image to image and after a
  reload. **Calibration** opens on its own only for a light with a Poor or
  Check finding, or when you came from the 🧪 chip or Images filtered by
  calibration; folded, it reads "no findings".
- Moving between images with the arrow keys keeps the detail's sidebar on the
  panel you were reading, even when you press the key the moment you stop
  scrolling or the browser is too busy to draw. In Safari and the macOS app
  it used to jump up to the panels above whenever the next image's sky
  context was loading.
- On a phone, an image's detail view sizes the image to the frame's shape:
  a landscape frame spans the width with no black band above and below,
  which leaves about 130 px more of the screen for the panels. A portrait
  frame stops at 60% of the height.
- Images and Sequence can show the newest frames first: **View › Sort ›
  Newest first** turns each group, the Sequence strip and its session tabs
  around, while the score chart keeps time left to right. The arrow keys,
  Shift-click, the detail view's next and previous, and Compare follow the
  order on screen. Flipping a view you have not moved in keeps the page
  where it is and puts the cursor on the new first card; once you have
  moved, the current image, the selection and its place on screen stay put.
  A project's exposure groups keep their order either way, so they do not
  trade places as frames arrive. The address keeps it as `order=newest`,
  the View button shows it, and the choice is remembered as the default for
  links that name no order.
- Signed in to a server with user accounts, your view choices follow you to
  any browser: the sort order, the grouping, the thumbnail sizes, the chips,
  the Library's rows and the project picker's grouping. A link still wins
  over them. Each change saves only what it changed, so a tab left open
  never undoes a choice made since on another device, and a change made
  while your choices are still loading is kept. The first account to sign
  in on a browser takes the choices it made before; later accounts start
  from the defaults. When your saved order arrives after the frames have
  drawn, a view you have not moved in puts the cursor on the new first card
  and stays at the top. A viewer saves theirs too. Without accounts, and in
  the desktop app, the browser keeps them as before.
- Images and Sequence each keep their own thumbnail size: moving between
  them from the header no longer carries one view's size to the other.
- A link that names open groups the view no longer has, such as an All
  projects link from before All projects grouped by project, opens the
  default groups instead of none.
- Sequence's **Scoring** penalties and reject limits have defaults of
  their own, in **Settings › Review › Sequence scoring**, with **Reset to
  built-in defaults**. Every view starts from them. A change in the toolbar
  now applies to that view only: the address keeps all five values, the
  control reads **Scoring (changed)**, and its popover offers **Save as
  default** and **Reset to default**. Images, the detail view and Stacks
  in the same project follow the change and say so, Stacks beside
  **Build**; another project starts from your defaults. Signed in, the
  defaults follow you to any browser like your other view choices. The
  values the control remembered before become your defaults. Min stars
  now takes a whole number.
- Frames captured since your last visit to a project from the Library carry
  a small dot before their date in Images and Sequence: the frames the
  Library's "new" badge counted, plus any captured since. They keep it until
  you next open the project from the Library.
- Form fields match the dark theme: text, number and date fields, menus and
  text boxes are dark with light text everywhere, such as Settings and the
  Sequence **Scoring** limits, even when the computer is set to light mode.
  Their arrows, date pickers, checkboxes and sliders follow the theme too.
- **Apply processing** in Stacks no longer sits on a bare "Applying…" while
  another stack job holds the worker. It says what it waits for, such as
  "Waiting for the stack build of M44 · Ha to finish", then the step and how
  far it has got. Stretch, deconvolution and RC-Astro runs wait in the same
  line as builds and appear in the header's background jobs, where they can
  be moved, taken out of the line, or, for an RC-Astro run, stopped; a row
  opens the target's stacks. Processing runs survive a restart: a server
  stop kills a running RC-Astro tool instead of waiting for it, and after
  the restart the run comes back in its place and resumes after the last
  tool step that finished. A page left open picks it up again.
- Calibration checks can be tuned in **Settings › Calibration**: the
  matching tolerances, the dark-master choices, every light check and every
  frame-grade limit. A light check can flag, be advisory (shown as Check,
  never "can't calibrate", so stacks keep the light) or be off. Under
  **Rigs**, any of them can be set for one rig, such as a rig that shoots
  sky flats or can't cap for darks, while everything else inherits; presets
  for sky flats, a dark library and no darks fill the form to review. A
  light's Calibration panel names the limits it was checked by, who set
  them, and what a setting kept from being flagged, and opens its rig's
  settings. The defaults are the limits used before. WBPP exports and runs,
  and `psf-guard export`, match by the same settings. With **Leave out
  lights that can't be calibrated** on, every export leaves out the lights a
  stack would, in either layout, from the CLI and in the AstroBin export,
  and says how many and why. The WBPP runner scripts list the settings in
  force and warn about frames an earlier export left in the folder. An
  older PSF Guard that saves its settings drops the rigs' values and the new
  settings, so set them again after a downgrade.
- A rebuilt stack, whether an automatic refresh or **Rebuild**, gets the
  processing its card showed: the same stretch, deconvolution and RC-Astro
  tools run again on the new stack once the refresh settles, as automatic
  work behind anything you start, with no page open. Until then the card
  shows the previous result, marked "From the previous stack ·
  reprocessing" with the run's progress, and the header lists the run as
  automatic. Only the newest stack is processed, and one that fails keeps
  the previous result, says why, and is not tried again. **Re-apply
  processing when a stack refreshes**, under Settings → Stacking, is on by
  default.
- A key pressed straight after opening, stepping or closing an image's
  detail or Compare now reaches the view you see. Before, → pressed just as
  the detail opened was lost, or moved the Sequence cursor behind it; ← →
  pressed quickly in a row could step one image instead of two; and →
  pressed just as the detail closed opened the next image again instead of
  moving the Images cursor.
