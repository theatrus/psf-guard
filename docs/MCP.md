# MCP server for agents

A running PSF Guard server is also a Model Context Protocol (MCP) server.
An agent such as Claude Code can list catalogs, read grades and quality
evidence, score sequences, apply grades, and start imports, quality scans
and WBPP runs through it. The endpoint is `/api/mcp` on the same port as
the UI. It uses the streamable HTTP transport, stateless, with JSON
answers, so a reverse proxy needs nothing extra.

## Connect

On a server with user accounts, open **Settings › Users › API tokens** and
choose **Connect an agent**. It mints a token of your own (read only unless
you tick **Let it grade and start jobs**) and shows the setup to paste for
Claude Code and for Codex, with this server's URL. **+ New token** mints one
with a label, expiry or user of your choosing, and shows the same setup.
From the CLI:

```bash
psf-guard users token create editor --label "claude on laptop" --expires-days 90
```

The token is shown once. Send it as a bearer credential.

For Claude Code:

```bash
claude mcp add --transport http psf-guard https://guard.example/api/mcp \
  --header "Authorization: Bearer psfg_…"
```

For Codex, add the server to `~/.codex/config.toml` and give it the token
through an environment variable, so the token never sits in the file:

```toml
[mcp_servers.psf-guard]
url = "https://guard.example/api/mcp"
bearer_token_env_var = "PSF_GUARD_TOKEN"
```

```bash
export PSF_GUARD_TOKEN=psfg_…
```

Any MCP client that speaks streamable HTTP works the same way: URL
`https://guard.example/api/mcp`, header `Authorization: Bearer psfg_…`. The
URL must end in `/api/mcp`.

A loopback server with no accounts, which is what the desktop app and
`psf-guard server` on a developer machine run, needs no token. Its
**Settings › Agents** tab shows the setup:

```bash
claude mcp add --transport http psf-guard http://127.0.0.1:3000/api/mcp
```

In the desktop app, choose **Settings › Server › Fixed port**, save it, and
restart the server to keep this URL across restarts. This sets the port for
the whole desktop HTTP server, including browser access and every API, as
well as MCP. **Agents** links to the same Server controls. If the port is in
use on a later start, the app reports the conflict and lets you change it
instead of choosing another port. The CLI server already supports `--port`
and `[server].port` in its TOML configuration.

A network server with no accounts answers 401 here as it does everywhere
else. See [server authentication](AUTHENTICATION.md).

## What a token may do

A token acts as its user. An editor's token can grade and start jobs; a
viewer's token, or any token minted **read only**, can only read. Every tool
sends its request back through the same `/api` router the UI uses, as the
caller, so the read-only role, `--allow-database-management` and each
handler's own checks apply to an agent exactly as to a person. A refusal
comes back as a tool error with the API's own message, so an agent can
explain it.

A token cannot mint or revoke tokens. That takes a browser session.

## Tools

Catalog tools take `database`, the id (slug) or name from
`list_databases`; Planning tools take a `plan_id` from `list_plans` or a
`rig` from `list_rigs`. Results are JSON text, the same shapes the UI API
returns. When no tool fits, `api_routes` lists every route `api_get` can
read, below `/api`, as you.

| Tool | What it answers | Needs write |
|---|---|---|
| `list_databases` | Open catalogs: id, name, path, image folders | |
| `list_projects` | Projects with targets, plans, progress, recent frames | |
| `list_targets` | Targets with coordinates, grade counts, last capture | |
| `list_images` | Lights with grade, filter, exposure and stored metrics; filter by project, target, grade and filter name; keep only the `metadata_keys` asked for; page with `limit` and `offset` | |
| `get_image` | One image: grade, location, header metadata, metrics | |
| `get_image_quality` | Score, issues, and place in the sequence, from stored evidence | |
| `analyze_sequence` | Relative scores and suggested rejects for a target, project, or the database | |
| `get_statistics` | Whole-database counts | |
| `get_calibration_report` | Calibration frames the library matches to a project's nights; rejected lights are left out unless `include_rejected` is set | |
| `list_stacks` | A project's latest stack per channel: frames integrated and left out, exposure, the masters each calibration session applied, the SNR outlook, and the `job_id` and `group_index` the other stack tools take | |
| `get_stack` | One stack channel frame by frame: sensor temperature, disposition and reason, registration shift and rotation, weight, noise, normalization; and per night, whether the frames were dithered or walked one way | |
| `get_stack_image` | The stack's preview as an image: as the app shows it, or with the background stretched hard (`stretch: "background"`); crop by fractions of the frame, up to 2048 px | |
| `get_stack_calibration` | The masters a stack applied, per session: frames each was built from, with their sensor temperatures and nights, outlier rejection, the lights it calibrated | |
| `explain_calibration` | Why one light got, or missed, each master: for bias, dark, dark flat and flat, every frame the library holds for its camera by night, with sensor temperatures, used, matching but unused, refused with the readings that disagree, or taken though graded suspect; what each master would span; and the light's calibration health findings, each naming the kinds it is about. For flats with measured grouping, the family and how it was chosen (alike range included), the side of a change the flats came from, and a person's choices and the options for one. `kinds` keeps only some kinds, such as `["flat"]` | |
| `get_flat_groups` | Each rig's flat groups: limits, angle term, the rig's clock; per family its groups by night with each session's `response_key`, the splits and joins between groups with why and the dust counted by kind and shape with the deepest few, a person's split or join and whether the measurements agree; change events, unmeasured sessions, orphaned choices. Narrow by `project_id` or `rig`; `full` returns every dust outline | |
| `get_flat_group_use` | What each flat group feeds: its flat masters, and the lights that take it by how their flats were chosen and by project | |
| `get_calibration_project_scope` | What one project's lights use from the library: sessions, masters, rigs, headers still being read | |
| `list_calibration_sessions`, `list_calibration_frames` | The library a page at a time, narrowed by project, rig, kind, filter, session, flat group, grade, kept out or nights | |
| `get_calibration_frame` | One frame: grade, findings, measurements beside its session's, and for a flat its spots and large dust, its session's dust and which of it this flat shows | |
| `get_light_flat_options` | The changes some lights were shot during, with each change's key and the sides already chosen | yes, as in the app |
| `get_calibration_duplicates` | The duplicate check or repair, and its plan a page at a time, with the plan's `token` | |
| `get_sky_coverage` | Every target's footprint and exposure by filter | |
| `get_jobs` | One database's jobs: import, quality backfill and scan, WBPP, calibration grading, flat measuring, frame sites, and the duplicate check or repair, each with what it waits for | |
| `astrobin_csv` | The AstroBin acquisition CSV for a project or target | |
| `grade_images` | Set `accepted`, `rejected` or `pending` with a reason; returns the grades it replaced | yes |
| `start_quality_backfill` | Start the background quality scan | yes |
| `start_import` | Scan the configured folders for new lights and calibration frames | yes |
| `start_wbpp_run` | Run PixInsight WBPP on a project's or target's accepted lights | yes |
| `cancel_wbpp_run` | Stop the running WBPP run | yes |
| `get_project_scheduler` | A project's Target Scheduler settings, targets and exposure plans | |
| `get_calibration_library` | Calibration totals: frames and masters, and per rig the count of each kind and the oldest and newest capture | |
| `get_activity` | Stack builds, scheduled refreshes waiting, and WBPP runs across databases; the backup under way and the last one, when you may read backups | |
| `get_backups` | The backup schedule, the archives (no download paths), the run under way and the last one | yes, plus management |
| `list_plans` | Planning: every plan with its target, rigs, goals and state | |
| `get_plan` | One plan: framing, rigs, exposures per band | |
| `get_plan_progress` | Frames per rig and objective against the goals | |
| `get_framing` | The framing draft: centre, rotation, mosaic panels | |
| `get_activation` | What was last written to the rigs, and whether the plan changed since | |
| `list_rigs` | Rig profiles and live status | |
| `get_rig_preferences` | A rig's effective observing preferences, and where each value comes from | |
| `get_rig_scheduling` | The Target Scheduler limits a rig would write to each project | |
| `list_templates` | The exposure template library, or a rig catalog's templates | |
| `search_sky` | Find a target by name or designation | |
| `api_routes` | Every route `api_get` can read, with what it answers | |
| `api_get` | Read any of those routes, as you | |
| `create_plan`, `rename_plan` | Start a plan, or rename one at the revision you read | yes |
| `save_plan`, `save_framing` | Save the whole plan or framing object, changed, at the revision you read | yes |
| `take_plan_from_target_scheduler` | Take a plan's values from what a rig's Target Scheduler database holds | yes |
| `check_feasibility` | When the target is observable from each rig's site over the coming nights | yes |
| `preview_activation` | Exactly what activating a plan would write to each rig, with a `preview_digest` | yes |
| `apply_activation` | Write that preview to the rigs; refuses if anything changed since | yes, plus management |
| `push_activation` | Send an activated plan to remote rigs | yes, plus management |
| `apply_rig_scheduling` | Write a rig's scheduling limits, with the digest `get_rig_scheduling` gave | yes, plus management |
| `save_template`, `delete_template` | Change the shared exposure template library | yes |
| `save_preferences` | Save observing preferences for global, a site or a rig | yes |
| `start_stack`, `cancel_stack` | Build stack previews for a project, target or filter; stop one | yes |
| `update_project`, `update_target` | Change scheduler settings; send only the fields to change | yes |
| `update_exposure_plan`, `create_exposure_plan` | Change or add an exposure plan | yes |
| `preview_reject_removal` | The rejects removal would take out, with a `digest` | yes |
| `apply_reject_removal`, `restore_removed_rejects` | Remove those rejects to the trash with the digest; bring a batch back | yes |
| `preview_sync`, `apply_sync` | Preview a pull or push between two open databases; apply it | yes |
| `set_calibration_validity` | Mark calibration frames for later or earlier lights, or clear the mark | yes |
| `solve_astrometry`, `predict_satellites` | Plate-solve a light; predict satellite crossings and check them in the pixels | yes |
| `check_uncalibrated_lights` | A project's lights the library cannot calibrate properly, by night, filter and reason, with a `digest` | |
| `reject_uncalibrated_lights` | Reject them with that digest, so Target Scheduler shoots them again | yes |
| `set_calibration_frame_choice` | Use anyway, keep out, or clear the choice on calibration frames | yes |
| `set_flat_group_override` | Split a flat family's groups at a session, join two groups, or reset the choice | yes, plus management |
| `set_light_flat_choice` | Put lights before or after a change, give them one group's flats, or reset their choices | yes, plus management |
| `stop_flat_measuring`, `resume_flat_measuring` | Pause flat measuring, or take up what was paused | yes |
| `stop_frame_sites`, `resume_frame_sites` | Pause reading frame sites, or take it up again | yes |
| `check_calibration_duplicates` | Check the library for exposures recorded more than once; writes nothing | read access, as in the app |
| `repair_calibration_duplicates` | Merge them, with the token of a check | yes, plus management |
| `stop_calibration_duplicates` | Stop the check or repair; a repair rolls back whole | yes |
| `start_backup`, `stop_backup` | Back up now, never with secrets, to the download area or the schedule's folder; stop the run | yes, plus management |

Jobs return at once. Poll `get_jobs` for one database's jobs, or
`get_activity` for stack builds, WBPP runs and backups across databases.

The server's instructions tell the agent the ground rules PSF Guard keeps
for people: `analyze_sequence` and `get_image_quality` suggest, and nothing
changes until `grade_images` runs; catalog predictions and header values are
not pixel evidence.

## Changing a plan

Plans change the way the app changes them. Read the plan (`get_plan`) or its
framing (`get_framing`), change the object, and save it whole with
`save_plan` or `save_framing`; a save made since you read it refuses, so
read again and redo the change. Saving changes nothing on a rig. To put a
plan on the rigs, `preview_activation` shows exactly what each rig's Target
Scheduler database would get and returns a `preview_digest`;
`apply_activation` writes it only if nothing changed since that preview, and
`push_activation` sends it to rigs on other machines. Both need write access
and a server started with `--allow-database-management`, as the app does.

## Looking into a stack

A typical question is "where does this pattern in the SII stack come from?"
`list_stacks` gives the channel's `job_id`. `get_stack_image` with
`stretch: "background"` shows the pattern; a crop shows it at full detail.
`get_stack_calibration` shows whether some sessions had no flat or a dark
master built from two frames, and the temperatures those frames were shot
at; `get_stack` gives each light's to set against them. `explain_calibration`
on one of their lights says why: a flat set marked for later lights, flats
at another rotator angle, darks of another exposure, or darks shot warmer
than the light. `get_stack` also shows, per night,
whether the frames were dithered: frames that walk one way without dithers
turn anything fixed to the sensor into streaks.

## Flats and their changes

"Why did these lights get last month's flats?" `explain_calibration` with
`kinds: ["flat"]` names the family, the change the light sits beside and
the side its flats came from. `get_flat_groups` shows the rig's groups and
why each split: dust, light pattern or vignetting against the limits, with
the dust counted by kind (new, gone, moved) and shape (compact, disk,
donut, patch) and the deepest few named. Dust that moved only as far as
focus travel explains is counted apart and does not split groups.
`get_calibration_frame` shows one flat's own dust beside its session's.

A person's choices change which flats lights take, and need write access
and database management as in the app: `set_flat_group_override` splits or
joins groups by the sessions' `response_key`, and `set_light_flat_choice`
puts lights on a side of a change by its `key`, which
`get_light_flat_options` lists for a set of lights. A reset names the
choice by its `anchor_frame`. Use anyway (`set_calibration_frame_choice`)
needs write access only.

A duplicate repair needs a check first. `check_calibration_duplicates`
starts it; `get_calibration_duplicates` shows the plan and its `token`;
`repair_calibration_duplicates` takes that token and applies nothing if
the library changed since, saying what changed.

## Backups

`get_backups`, `start_backup` and `stop_backup` need write access and
database management, as Settings › Backups does. An agent never handles
secrets: `start_backup` refuses `include_secrets`, whoever signs in, and
writes only to the download area or the folder the schedule already uses,
not a folder of its choosing. Archives are listed without download paths,
and `api_get` refuses the download route. A person downloads an archive,
or makes one with secrets, from Settings. Restore is not offered.

## Try it with curl

```bash
TOKEN=psfg_…
curl -s https://guard.example/api/mcp \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call",
       "params":{"name":"list_databases","arguments":{}}}'
```

## Notes for operators

- The URL must end in `/api/mcp`. The bare host serves the app: it answers
  a POST with 405 and a note giving the right URL, so a client given the
  wrong URL fails with that message.
- The endpoint sits inside the API router, so the login middleware, the
  read-only role, and the database-management gate apply to it as they do
  to the UI.
- Requests are stateless. There is no MCP session to expire; revoke the
  token to cut an agent off, and it stops with the next call.
- Revoking a user's account revokes that user's tokens.
- Tokens live in `auth.json` beside the database registry as SHA-256
  hashes. The CLI edits the file; restart the server for a CLI change to
  apply. Tokens minted in Settings apply at once.
