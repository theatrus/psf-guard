# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

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

- The Director page is now a plan list: every project with the databases it
  is linked to, how far its framing, plan and activation have come, and a
  Rig planning link, above a rig list with each database's planning state,
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

- Rig planning can now activate a project: after a preview, PSF Guard writes
  one Target Scheduler project per participating rig, a target per mosaic
  panel and an exposure plan per objective into that rig's database, and
  updates the same rows on later activations without touching captured
  frames or grades.

- Rig planning gains a plan below the framing: objectives per bandpass in
  accepted hours or frames, and per rig the exposure template and exposure
  length that meet them, with frames worked out per rig and defaults that
  follow each rig's optics and sky. Plans save as drafts on the project.

- Rig planning now frames a project on the sky. Below the project editor, a
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
  Apply, instead of a separate Director Catalogs view. Overview's existing
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

- An experimental Director page manages global projects, sites, and rigs on
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

- The Director page opens with a **Live** table of rigs: whether each is
  online, quiet, offline or never seen, from the server's own record of the
  plugin's calls; what it last reported it was doing, marked stale when old;
  how long since it pulled its program, checked in and reported; saved frames
  awaiting grading; and the plans it is assigned to.

## Changed

## Fixed
