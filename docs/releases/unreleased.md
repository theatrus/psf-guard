# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- The quality scan now measures stars the way each rig's N.I.N.A. did, so
  its star counts and HFR compare with the values N.I.N.A. recorded.
  Choose how in the new **Star detection** section of **Settings › Review ›
  Grading & display**: a **Default** row, and one row per rig with
  **Detector**, **Sensitivity**, **Noise reduction** and **Hocus Focus
  version**. The defaults match N.I.N.A.'s own: **Auto**, which uses Hocus
  Focus for frames where N.I.N.A. recorded a Hocus Focus FWHM, **High**
  sensitivity, no noise reduction, and Hocus Focus 4. N.I.N.A.'s detector
  measures each frame as the N.I.N.A. release that wrote it does. After
  updating, and after you change a rig's settings, the next quality scan
  measures the affected frames again; until then, scoring does not use
  their earlier scan values.
- PSF Guard can also measure each frame with its N.I.N.A. profile's full
  star detection settings: the autofocus crop and brightest-star count,
  the autostretch, and Hocus Focus's own options. Import a profile file
  under **Import N.I.N.A. profile** in the same section, for all frames
  or from a date. The PSF Guard Sync and Director plugins send them as
  they capture, and a frame taken before the first record uses it. With
  their profiles imported, three Hocus Focus rigs matched the star counts
  and HFR N.I.N.A. recorded to within 0.2%.
- Max HFR and Min stars now judge the values you choose. A frame can carry
  two star counts and HFRs: the ones N.I.N.A. recorded during capture and
  the ones PSF Guard's quality scan measured. They come from different
  detectors and can differ several-fold. Before, the limits always judged
  PSF Guard's values while every view showed N.I.N.A.'s, so a limit set
  from the numbers on screen rejected the wrong frames. Image Information
  now shows both values, cards show the one in use, and a small **N** or
  **P** marks each value's source. Choose the source under **Stars and
  HFR** in the Scoring control, or **Measured by** in Settings › Review ›
  Sequence scoring. PSF Guard stays the default. The choice also sets which
  values frames are compared on and which the Stars and HFR cutoff uses. A
  reject reason now says which source its value came from and shows HFR to
  two decimal places.
- On Windows, PSF Guard keeps each storage volume under its **Volume usage
  limit**, as it does on macOS and Linux. When a volume passes the limit,
  PSF Guard deletes image previews least recently viewed first, then stack
  resume checkpoints a day old or more, then calibration masters unused for
  a week that it can build again. Before, this cleanup never ran on
  Windows, and **Disk use** in **Settings › Storage & backups › Folders &
  limits** showed "The first check runs a minute after the server starts."
  in place of each volume.
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
  schedule. A night with no clear dark hours is red, including one a
  forecast leaves clear only in daylight. The server keeps the night plans
  and computes them again when a mark, a forecast, a rig's setup or its
  database changes, as a person's work the header lists, so the page
  rarely waits; **Updated** shows when they were computed. An AI agent
  reads the same plans with the MCP tool `get_night_plans`. See
  [Nights](../DIRECTOR.md#nights).
- A site can mark its nights from a cloud forecast. Under **Settings ›
  Presets › Sites**, choose **Open-Meteo** or **MET Norway** in **Forecast**, and the
  cloud cover at or under which an hour counts as clear. Night plans then
  treat cloudy hours as not available for every rig at the site, unless a
  mark you set for the site overrides the forecast. The **Nights** grid
  shows a cloud on each night the forecast marks, and a night's timeline
  shows each hour's cloud cover. Forecasts are off by default: PSF Guard
  sends a site's location, rounded to about 1 km, only after an editor
  turns its forecast on. See [Cloud forecasts](../DIRECTOR.md#cloud-forecasts).
- A night's panel under **Rigs › Nights** says who decides the rig's
  night, and lets you change it:
  - On a rig Director drives, **Edit night** edits the night on its
    timeline: choose a block to pin it or leave it out, drag it to a new
    time, drag pins and their edges, drag the hold line in from the right,
    and set the night's plan order. The schedule shows each edit before it
    is saved, and a session already running takes saved overrides at its
    next check-in. Overrides only narrow the program the rig pulls, and
    lapse when the night ends. See
    [Override a Director rig's night](../DIRECTOR.md#override-a-director-rigs-night).
  - On a rig Target Scheduler runs, **Target Scheduler settings** sets each
    project active or not with its priority, and each target active or
    not. The schedule plans the change first; saving writes it to the rig's
    database, and to Target Scheduler's own file for a catalog kept in step
    with it, on a server that may write into rig databases. Each row is
    saved only while it still holds what was read, and only rows of the
    rig's N.I.N.A. profile. These last until changed back, for every night.
    See
    [Change a Target Scheduler rig's settings](../DIRECTOR.md#change-a-target-scheduler-rigs-settings).

  **Mode** in the rig's setup sets who runs it, Target Scheduler or
  Director; until it is set, PSF Guard guesses from what the rig has
  reported, and pairing a Director plugin sets it to Director. A site's new
  **Time zone** under **Settings › Presets › Sites** sets the clock marks
  and overrides are entered and shown in.
- The schedule under **Rigs › Nights** shows the closest the Moon comes to
  the target on each filter run. The angle is dimmed while the Moon stays
  down. A strip under the schedule shows when the Moon is up and how full
  it is. Choose a block to see the Moon's closest angle and highest
  altitude for each of its filter runs.
- **Visibility** in a plan's workspace estimates the nights a rig needs
  with the marks and the forecast counted, and shows each night's clear
  hours and marks. Once the plan is activated on a rig, it also shows the
  night the rig's own schedule finishes the plan, and the other plans that
  take the rig's time first. See [Visibility](../DIRECTOR.md#visibility).
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
