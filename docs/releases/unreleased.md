# Unreleased

- Set per-filter Moon avoidance in Director exposure templates. The shared
  planner skips blocked recipes, preserves Target Scheduler lunar settings and
  lets capable NINA sessions wait parked when only Moon restrictions block work.

- Allow Director executors to request bounded multi-target workloads for local
  priority scheduling, while preserving the original prepared-target mode and
  the same commissioning, launch, budget and release checks.
- Preserve Target Scheduler project priorities in newly imported Director plans
  instead of assigning priorities by filter name. Existing drafts stay unchanged.

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- The job queue survives a restart: stack and color builds and automatic
  refreshes still settling come back in order, and waiting WBPP runs keep
  their place. A WBPP run the restart cut off shows as stopped rather than
  starting again from nothing. With automatic previews on, every database is
  checked once after a start, so frames that arrived while the server was
  down still reach their stacks.

- **Settings → Stacking → Processor use** sets the share of cores for work
  you wait on and for background work, without a restart. Automatic stack
  refreshes now use the background share instead of half the cores.

- The header's background jobs queue can change the order of waiting stack
  builds and WBPP runs, take one out of the line, and stop the running job
  after asking once. WBPP runs from every database appear there too, and so
  do automatic stack refreshes still waiting for new frames or grades to
  settle, with **Run now** and **Skip**.

- Stack progress keeps moving through the final rejection pass, in the header
  and on the stack card, instead of sitting at 100% while the three passes
  run.

- Director's experimental workload API can commission automatic requests for a
  reviewed rig/client and project scope, release a clean parked session after
  complete capture delivery, and issue successor work without resetting spent
  attempts or pending credit. Uncertain work remains blocked for reconciliation;
  commissioning UI and multi-target execution are still pending.

- Director can receive native equipment reports from paired NINA clients for
  explicit operator review. Accepting a fresh report preserves manual rig setup
  and refuses stale state or outstanding allocations; reporting alone never
  changes active equipment or grants acquisition. The review UI is still pending.

- Director's experimental allocation API supports a one-shot executor launch
  bound to the paired client and local ledger. A retry or lost local database
  cannot launch the same allocation again. Restart recovery remains a separate,
  unfinished reconciliation workflow.

- Director's experimental API can admit an immutable first allocation for a
  specific paired client. Retrying or restarting returns the same plan and
  attempt budgets; expiry, re-pairing and changed previews cannot replace it.
  Admission UI and successor reconciliation remain unfinished. The separate
  Director plugin can opt in to experimental prepared-target acquisition.

- Director program previews retain their identity and validity across retries
  and server restarts, fit the shared geometry core's 24-hour limit, and report
  unsupported template settings instead of silently substituting camera values.
  Native filter labels can map templates to stable NINA filter IDs; inactive or
  detached targets and unactivated plan edits cannot become capture candidates.

- Quality scans now measure each frame's photometric zero point against Gaia
  stars and its sky noise, so SNR takes its place in the quality score beside
  star count and HFR. A frame dimmed by haze or shot under a brighter sky
  scores lower than frames with the same exposure, from any night. Existing
  frames need a quality scan with **Force** to gain it.

- A new **Stacking** page in Settings sets how stack previews integrate
  their frames. It defaults to Seiza 0.19's recommended method: local
  background normalization with no frame-edge seams, frames weighted by
  their noise, a reference chosen from each frame's stars and sky, quadratic
  registration for lens distortion, and Lanczos-3 resampling. **Draft** skips
  the final rejection pass for a quick look, and **Classic** keeps the old
  method. Bayer drizzle is there for dithered color frames. The weighting
  checkbox has left the stack panel, which now names the method in use and
  marks stacks built another way out of date. Automatic previews moved to the
  same page. `psf-guard stack-snr --weight-by-noise` weighs frames from the
  command line. Stacks built before the upgrade show as out of date until
  they are rebuilt.

- The visibility chart marks the target's meridian transit and draws the
  rig's meridian pause as a break in the track; the hours lost to it are
  taken out of the visible time and named in the verdict.

- Framing now behaves like N.I.N.A.'s framing assistant: a rectangle for the
  rig's field appears at once (rigs take their optics from their own frames
  when they are adopted), you drag it to move the target and its handle to
  turn it, quarter turns and the rig's fixed camera angle are one click, a
  compass and a readout show orientation, coordinates, angle and extent.

- Mosaic plans can give each rig its own panels: a wide rig takes the whole
  field while a long-focus rig takes a corner. Activation writes only the
  panels a rig owns, and both the plan and the activation preview name any
  panel no rig covers.

- The framing view now says whether the target is visible tonight from each
  rig and for how long, and draws the night's altitude chart with the rig's
  custom horizon, the Moon and the darkness bands, with a week of nights and
  an estimate of how many nights a rig needs for its share of the plan.

- Director now takes every registered database in as a rig and every project
  in it as a plan on its own; projects that share a Target Scheduler GUID
  across databases are one plan with several rigs. The project planning
  links panel is gone from Settings. Each plan opens a workspace with its
  databases, framing, the rigs that shoot it, and activation; a project with
  no database yet starts its framing from a name looked up in the CDS
  catalogs or typed coordinates. Rig setup expands in place from the rig list.

- Director clients can pair using one-use operator-issued codes, with separate
  per-rig and per-NINA-profile credentials for program inspection and receipt/status
  reporting. Operators can list and revoke clients; pairing does not authorize
  equipment control or acquisition.

- The Planning page (then labelled Director) is now a plan list: every project with the databases it
  is linked to, how far its framing, plan and activation have come, and a
  Open in Planning link, above a rig list with each database's planning state,
  optics and last plugin status. The old Projects, Sites and Rigs tabs are
  gone; their links land on the same page.

- The Director plugin can check in its capture receipts and report live
  status over the API. Receipts are stored once and acknowledged by cursor,
  saved frames count as pending credit until grading accepts them, and both
  calls tell the plugin when to pull a new program.

- The Director plugin can now pull a rig's program over the API: one goal per
  activated exposure plan with the frames still owed, the panel targets and
  the recipes bound to the filters the rig reported, valid for 36 hours per
  pull so a rig keeps working through a network outage.

- Planning can now activate a project: after a preview, PSF Guard writes
  one Target Scheduler project per participating rig, a target per mosaic
  panel and an exposure plan per objective into that rig's database, and
  updates the same rows on later activations without touching captured
  frames or grades.

- Planning gains a plan below the framing: objectives per bandpass in
  accepted hours or frames, and per rig the exposure template and exposure
  length that meet them, with frames worked out per rig and defaults that
  follow each rig's optics and sky. Plans save as drafts on the project.

- Planning now frames a project on the sky. Below the project editor, a
  survey image (DSS2, SDSS, DESI Legacy, 2MASS, the Finkbeiner H-alpha map
  and the Northern Sky Narrowband Survey layers, as in N.I.N.A.'s framing
  assistant) shows the target with its catalog coordinates and rotation; drag
  to pan, scroll to zoom, pick the rig whose field makes one panel, lay out a
  mosaic with overlap, overlay other rigs' fields, and save the framing as a
  draft on the project. Images are cached on the server for offline replanning.

- Each planning-enabled database now has a rig profile under its project
  planning links: sensor and optics, site, sky quality, altitude and meridian
  limits, with a one-click fill from the newest frame's headers and a live
  field-of-view readout. The N.I.N.A. Director plugin can report the same
  values, and the card says where each one came from.

- Director planning is on by default. The server and the desktop app keep the
  planning store in `director-meta.sqlite` beside the database registry
  whenever database management is allowed; `--director-meta` still moves it.
  A read-only server leaves Director off.

- Director's rig list now uses registered project databases. Reviewed setup
  binds each database to one rig across its source profiles, preserves existing
  unambiguous links, and reports conflicting prototype links without rewriting
  history. Separate databases can contribute to one shared project.

- Database settings now hold project planning links with explicit preview and
  Apply, instead of a separate Director Catalogs view. Library's existing
  project plan dialog opens rig planning with the same targets, coordinates and
  exposure templates, and returns to the original project and database scope.
  Framing, downstream plan generation and combined progress remain in development.

- Director's experimental operator API can preview and apply confirmed catalog
  project/profile links to global projects and rigs. It preserves catalog
  identity across moves and retries without changing TS history or image grades.
  Acquisition remains unavailable.

- Director's experimental API can inspect registered catalogs for projects and
  profile IDs, reporting missing or duplicate identities without modifying the
  source database or reading image data.

- An experimental Planning page (then labelled Director) manages global projects, sites, and rigs on
  servers with Director metadata enabled. It supports read-only accounts,
  paged listings, and revision-checked renames without requiring a catalog.

- An opt-in Director metadata API can create, list, and rename global project,
  site and rig identities, and retain complete immutable horizon and equipment
  snapshots in a separate SQLite store. Enable it with `--director-meta` and
  database management on a CLI server. It follows user authentication and does
  not yet enable pairing or acquisition. See `docs/DIRECTOR.md`.

- The server is an **MCP server** at `/api/mcp`, so an agent such as
  Claude Code can list catalogs, read grades and quality evidence, score
  sequences, apply grades, and start imports, quality scans and WBPP runs.
  Write tools follow the caller's role, and the WBPP tools follow the
  database-management gate as the UI does. See `docs/MCP.md`.
- **API tokens** for scripts and MCP clients: mint one under **Settings →
  Users → API tokens** or with `psf-guard users token create`, send it as
  `Authorization: Bearer psfg_…`. A token acts as its user, can be made
  read only or given an expiry, and is shown once. Revoke it in the same
  place or with `users token revoke`; removing a user revokes their tokens.

- A rig at another site can be planned from here. Set **Remote site** on
  the rig's profile to the Sync peer that holds its real database; Apply
  then writes the plan into the local copy and pushes the same Target
  Scheduler rows to the peer, and **Push to remote sites again** resends the
  last activation when a site was offline. The activation report says per rig
  whether the peer took the rows.

- The framing view shows the panels a plan has already shot: each activated
  panel's latest stack is drawn on the sky where its plate solve puts it,
  beside the planned rectangles, with a list of every panel's accepted
  frames and why a stack is missing. A checkbox hides the stacks.

- The plan list counts frames accepted against desired for every plan whose
  databases hold targets, and per target under each database, so a mosaic
  shows each panel's progress without opening it.

- The Planning page opens with a **Live** table of rigs: whether each is
  online, quiet, offline or never seen, from the server's own record of the
  plugin's calls; what it last reported it was doing, marked stale when old;
  how long since it pulled its program, checked in and reported; saved frames
  awaiting grading; and the plans it is assigned to.

- Director plans read like Library projects: the same card, frame counts,
  desired-progress and grading bars, with one database chip per rig and a
  Rigs list giving each rig its own progress and per-panel counts.
- The header's project picker shows a project shot by several rigs as one
  row, badged with its rig count, that opens to each rig's images and
  targets; typing any rig's name, database or target finds it.

## Changed

- The header keeps the views on its first row whenever they fit beside the
  brand and the utilities, and gives them a row of their own only when they
  do not, rather than below a fixed 1500 px. The **Stats** switch moved from
  the header to the Images grid, beside the image count it expands on.

- Background work in the header is one small chip: the overall progress and
  the number of jobs. Hover, focus or click it to see the queue, with every
  catalog refresh, quality scan and stack build, running or queued, on any
  database. It used to take a wide slot beside the views.

- Stack previews keep different exposure lengths apart by default, so 120 s
  and 300 s frames build separate stacks instead of dimming each other. A
  project can still turn **Separate exposure groups** off, and a project that
  chose either way keeps its choice.

- The Linux AppImage and .deb carry AppStream metadata: a summary, a
  description, links and four screenshots, so software centres and the
  AppImage catalog show PSF Guard properly instead of a first-run capture
  and the desktop file's one-line comment.

- Framing drags work both of N.I.N.A.'s ways: move the rectangle over a
  still sky (the default) or pin the rectangle and move the sky, and the
  target, under it; Shift-drag looks around without moving the target in
  either. **Turn the sky with the camera** keeps the rectangle upright and
  turns the sky by the camera angle, with the compass following. Both
  choices are remembered in the browser. The stage is a globe about the
  center of the view, as in N.I.N.A., so a drag turns the sky under the
  pointer and the survey picture turns with it.
- The framing view marks what is in the field: deep-sky objects from the
  Seiza object catalog at their size and angle, comets and asteroids from
  its minor-body catalog placed for the moment you look, and the Sun, Moon
  and planets from a built-in ephemeris. Each layer has its own switch and
  says when its catalog is not on the server.
- The framing view keeps tonight's visibility chart on screen: the sky
  takes the height left over, the visibility strip sits under it with the
  nights table folded away, and the column stays put while the form beside
  it scrolls. The drag mode (rectangle or sky), the sky turn and the layer
  switches (grid, constellations, deep-sky marks, comets and asteroids,
  Sun, Moon and planets) are small buttons at the top of the stage.
- The Library opens compact: one row of pills per project with its
  database, state, accepted-of-desired progress, grading counts and dates,
  and a plan shot by several rigs wrapped in an outer pill that sums them.
  A row opens into the full card on request and folds back with the
  arrow at its top left, in either view; the project you came from opens
  on its own until you fold it. **Compact** and **Detailed** switch the
  whole list, and Settings, Review, **Library** sets which one the page
  opens in.
- A plan can take over another plan's database project: under
  **Databases** in the workspace, **Attach a project from another database**
  lists projects in databases the plan has none in, and attaching moves
  them over and retires the other plan, keeping this plan's drafts. Two
  rigs that each made their own project for one target become one plan
  this way. **Detach** gives a database's project a plan of its own again.
- A rig in a plan can be framed on its own: its own grid, camera angle and
  panel size, drawn in a colour of its own beside the shared framing. It
  starts on the shared target center and follows it until you drag its
  rectangle, type a center, or move it to the view center; one tick puts it
  back in step. Activation writes that rig's targets from its own layout;
  the plan's panel picker and coverage check follow each rig's grid.
- Planning has a shared **Exposure templates** library. A plan can bind a
  rig to a library template when the rig's database has no template for the
  band, so a second or third database joins a plan without templates of its
  own; activation writes the template into that database under the
  library's identity. Copy a rig's templates into the library in one click,
  edit rows in place, and remove what is no longer wanted.
- The visibility chart arrives about ten times sooner (a week for two rigs
  now takes under a tenth of a second on the server, with the same numbers)
  and asks a third of a second after the target stops moving instead of
  more than half. Under the framing stage it is drawn at one unit per
  pixel, so its box is the chart and nothing more, with the rig, verdict,
  estimate and the folded legend and nights table beside it.
- Activation takes over the targets a project already has instead of
  creating twins beside them: the one with the panel's name, else the one at
  the panel's place, else the only target of a single-panel plan. The
  preview says `adopt` or `update` for them.
- Switching a plan goal between hours and frames converts the number
  through the rig's exposure length, so 6 h at 300 s becomes 72 frames and
  back again.
- Planning no longer answers "Director metadata is busy" when a workspace
  opens. Reads of the planning store run on a pool of read-only connections
  beside the one writer, so the eight requests a page fires at once are all
  served; only writes take turns, and only a write that has waited twenty
  seconds answers busy. Rig-database work (adoption, binding, activation) has
  a gate of its own and no longer holds the store.
- **Find a target** offers names as you type. The server's own Seiza
  catalog answers first, with no network: designations, common names and
  aliases, one row per object with its kind. The last row looks the name up
  online at CDS Sesame, and those answers are kept on the server so a name
  goes online once. Enter takes the highlighted row, an exact local match,
  or goes online, in that order. The start-framing form uses the same box.
- Marks start as the map an imager frames by: Messier, NGC, IC, Sharpless,
  LDN, LBN and the supernova remnants, drawn with thinner outlines. The other catalogs, PGC and HD
  above all, wait for a chip under **Catalogs marked** in **View**; they
  flooded every field with faint galaxies and stars and crowded out the
  nebulae and clusters. Comets, asteroids and the Sun, Moon and planets
  wait for their button too, since a zoomed-in field filled with faint
  asteroids. Every choice is remembered in the browser. Marks also follow the view sooner
  after a drag: they no longer wait for the survey tile to settle, and a tile
  arriving no longer pushes that wait back.
- Offline sky maps are preferred wherever one stands in for an online
  survey: a framing saved on the DSS2 plates or the online NSNS SHO layer
  opens on the offline set when the server has it, and plan thumbnails draw
  from it too; the online layer stays a click away. A newly listed set has
  its small tiles warmed in the background so the first wide view renders
  at once, and once a framing view has its tile the browser fetches wider
  views of the same place out to a hemisphere, so zooming out already has a
  picture. A view's own tile now follows the viewport: a little wider than
  the view at the screen's own pixel density, so the picture is sharp across
  the whole stage on a high-density display instead of only around the
  centre.
- The survey picture now follows the framing view at every zoom, out to a
  hemisphere, instead of giving way to a bare chart past 30°. Offline
  N.I.N.A. maps render a view in a fraction of a second rather than a second
  or two, composite every tile of the set for a wide view instead of the
  nearest forty-eight, and keep decoded tiles in memory between renders;
  the browser asks after a pending image sooner.
- The framing stage is drawn like the Sky view: it fills the column and most
  of the window, pans and zooms from three arcminutes out to a hemisphere,
  and carries an equatorial grid whose spacing follows the zoom, with
  declination labelled down the left edge and right ascension along the
  top, and constellation figures and names once the view is wide enough.
  Survey images and offline maps are now rendered in the same stereographic
  projection the stage draws in, and while the pointer moves the picture is
  laid in with the turn between its own plane and the stage's, so it stays
  under the grid and the rectangle. Earlier cached images are regenerated on
  first use.
- The Library's **Plan & coordinates** dialog is gone; its **Planning** button
  opens the project in Planning with that database's targets and exposures
  expanded under **Databases**. The same editor, with framing, plan and
  activation around it.
- Planning is on for every server. Without database management it is
  read-only over the catalogs: plans and framing can be drafted, and
  activation, pushes and Target Scheduler edits are refused. No table is
  written into a rig database until a managing server lists it, and the rig
  keeps the same identity when that happens.
- The header's views are now **Library** and **Planning**. Library is the
  page that was called Overview: every database's projects, images and
  grading. Planning is the Director operator page: plans across rigs, the
  framing workspace, activation and live rig status. The project dialog's
  **Rig planning** button is now **Open in Planning**. Routes and links are
  unchanged; Director remains the name of the plugin, protocol and API.
- The planning workspace is a wide screen: the framing sky fills the window
  up to the screen's height, with survey chips on it for the DSS2 colour and
  narrowband layers; plan and activation sit beside each other under it. The
  plan list shows each framed plan as a survey thumbnail with its panels.
- The framing rectangle and mosaic grid follow the pointer at once: they are
  drawn in the browser with the server's own geometry instead of waiting for
  a round trip on every change. **Find a target** in the framing controls
  looks a name up and moves the target there, with **Undo** and **Back to
  saved framing** as the ways back.
- N.I.N.A.'s offline sky maps work as framing layers. `psf-guard sky-maps
  install full|nsns-ohs|nsns-ohs-starless` downloads a whole-sky set into the
  server's cache, and the framing view renders any view from its tiles with
  no network; a new framing starts on the offline DSS map when one exists.
  Tiles are decoded by content, since N.I.N.A.'s DSS set keeps some small
  versions as PNG behind a `.jpg` name, and an unreadable file gives way to
  the next size up instead of failing the view.

## Fixed

- A queued WBPP run no longer vanishes from the line when its turn comes while
  its database is busy; it keeps its place at the head. Two WBPP starts that
  arrive together, or a start that races the line, can no longer launch two
  PixInsights at once: the server claims its one PixInsight slot in a single
  step.

- Darks that caught stray light, such as dawn through an open roof, no longer
  reach stack masters or WBPP exports. PSF Guard measures each dark once in
  the background, marks one that sits well above its matching darks, and
  names the frames it left out. The calibration library shows the mark.

- Color previews with background extraction no longer tint the corners. The
  automatic model now picks one background shape for all channels instead of
  one per channel, which had left some previews green in one corner and
  magenta in the opposite one.

- The survey picture under the framing view no longer stretches the wrong
  way while you pan and then jumps when the settled tile arrives. It is
  re-projected on the GPU for every frame from the tiles fetched so far,
  finest on top, so it sits under the grid and the rectangle at every zoom
  and turn, and a new tile only adds detail.
- Panning the framing view no longer moves the rotation handle off the
  rectangle's up direction. The handle and the angle it sets now live on the
  target's own tangent plane, where the camera angle is measured, so turning
  the field behaves the same however far the view has panned.

- Opening a plan no longer fails with "Director metadata is busy; retry
  shortly" when several of its requests arrive at once: a request waits its
  turn for up to twenty seconds instead of three, the visibility chart's
  night curves are computed after the store is released, and the browser
  waits through a busy answer instead of showing it as an error.

- Plans whose Target Scheduler project already has targets and exposure
  plans no longer read "Linked, not framed yet": Director imports them as
  its framing (with the mosaic grid the targets form) and plan drafts the
  first time it lists the project, so the workspace opens with them filled
  in and the list shows the framed sky.

- The catalog identity and Director tables PSF Guard adds to a rig database
  are plain SQL again, so N.I.N.A., Target Scheduler and older SQLite tools
  open the file as before; tables a preview build made `STRICT` are rebuilt
  in place with their rows the next time PSF Guard writes them.
- A database file copied by hand carries the original's identity. The
  Director now names the copy at the top of the plan list and leaves it out
  of planning instead of treating both files as one rig.

- A browser-user file (`auth.json`) written by a newer PSF Guard loads
  instead of stopping the server; fields this version does not know are
  kept and written back unchanged.

- Planning lists plans the way the Library lists projects: compact rows
  with the same pills for database, Target Scheduler state, progress that
  reads Done once the goal is met, grading and capture dates; a plan shot
  by several rigs is an outer pill with a row per rig, and a plan closed
  on every rig sits under an Archived plans fold. The arrow opens a row
  into its full card, and the Library's Compact / Detailed setting applies
  to both views.


- Planning can narrow its plan list: a Show plans select for Active,
  Inactive, Draft, Closed, Done, Still to shoot or No database, and a
  search box, both kept in the URL. Each rig's row links to that project
  in the Library and to its image grid, and on a server with database
  management the state pill is a select that changes the rig's Target
  Scheduler project state in place.

- The header now reads Library, then a **Review** group (the project
  picker with Images and Sequence) and, with Planning on, a **Plan** group
  (a plan picker with Workspace), then Sky. Images and Sequence wait until
  a project or database is chosen; picking one from the Library opens
  Images for it. A **Live** chip beside the job status counts the rigs and
  which are exposing, turns red when one has gone quiet, and opens the Live
  table from any view. Rig setup and the exposure template library moved to
  Settings › Rigs and Settings › Exposure templates.

- The Library now holds every plan, and the Planning list page is gone. A
  plan shot by several rigs shows its stage and a Planning button on its
  outer pill, a lone project opens its plan from its own row, and plans
  with nothing captured yet wait in their own section under the projects,
  where New plan and rename live. A **Show** select (Active, Still to
  shoot, Done, Draft, Inactive, Closed, and No database with Planning on)
  and the search box narrow the Library and stay in the URL, and a row's
  state pill changes the project's Target Scheduler state when the server
  allows database changes. Old Planning links land in the Library.

- A plan's workspace has its own address, `/plan?plan=<key>`. The key is
  the Target Scheduler GUID the plan's rigs share, so a link opens the same
  plan on any PSF Guard instance that holds those databases. Old Planning
  links still land on the right plan or in the Library.

- Live moved onto the Sky. The header's Live chip, or the Live chip in the
  Sky's controls, draws each reporting rig on the coverage map where it
  points now, lists the rigs beside the map (choosing one turns the map to
  it), and shows the full Live table under the timeline. A quiet rig is
  drawn dashed red. The drawer is gone.

- The last of the Planning page is gone: the workspace is headed **Plan workspace**,
  the Library's project cards offer **Plan** instead of Planning, and old
  `/director` links still forward to the right place.

- The header's two pickers became one scope, read left to right from the
  whole to the part: the project (a plan shot by several rigs shows its rig
  count), its **Workspace**, then which rig and target **Images** and
  **Sequence** show. A lone rig's project lists its targets there. Every
  view button marks the current page the same way, and the view strip is
  only as wide as its buttons.

- The header's button to a plan's workspace is called **Planning** again,
  as are the Library card's button and the page heading. A project with one
  rig and one target shows them as a captioned label beside Images, set
  apart from the buttons, rather than as bare text that looked clickable.

- Target Scheduler projects without a GUID can be planned again. Target
  Scheduler skips giving existing rows a GUID when a database is upgraded
  past its schema 22 in one step, and such projects were invisible to
  Planning. Settings › Databases now says how many rows are affected and
  offers **Fill in GUIDs**, which backs the database up beside itself and
  fills only the empty ones; `psf-guard fill-guids` does the same.
- A Target Scheduler mosaic far from the equator is imported as its grid
  of panels instead of a single panel.
- With a target chosen in the header, the stack previews show only that
  target's stacks and colour compositions, not every panel of a mosaic.

- The header's rig and target switcher is as wide as the chosen target's
  name, within reason, so long names show whole. Mosaic panels that share
  a long start show the part that sets them apart ("Panel 2"), with the
  full name as a tooltip.
