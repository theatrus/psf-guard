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
| `get_calibration_report` | Calibration frames the library matches to a project's nights | |
| `list_stacks` | A project's latest stack per channel: frames integrated and left out, exposure, the masters each calibration session applied, the SNR outlook, and the `job_id` and `group_index` the other stack tools take | |
| `get_stack` | One stack channel frame by frame: disposition and reason, registration shift and rotation, weight, noise, normalization; and per night, whether the frames were dithered or walked one way | |
| `get_stack_image` | The stack's preview as an image: as the app shows it, or with the background stretched hard (`stretch: "background"`); crop by fractions of the frame, up to 2048 px | |
| `get_stack_calibration` | The masters a stack applied, per session: frames each was built from, outlier rejection, the lights it calibrated | |
| `explain_calibration` | Why one light got, or missed, each master: for bias, dark and flat, every frame the library holds for its camera by night, used, matching but unused, or refused with the readings that disagree | |
| `get_sky_coverage` | Every target's footprint and exposure by filter | |
| `get_jobs` | Import, quality scan and WBPP progress | |
| `astrobin_csv` | The AstroBin acquisition CSV for a project or target | |
| `grade_images` | Set `accepted`, `rejected` or `pending` with a reason; returns the grades it replaced | yes |
| `start_quality_backfill` | Start the background quality scan | yes |
| `start_import` | Scan the configured folders for new lights and calibration frames | yes |
| `start_wbpp_run` | Run PixInsight WBPP on a project's or target's accepted lights | yes, plus management |
| `cancel_wbpp_run` | Stop the running WBPP run | yes, plus management |
| `get_project_scheduler` | A project's Target Scheduler settings, targets and exposure plans | |
| `get_calibration_library` | The calibration library by night, with validity marks and masters | |
| `get_activity` | Stack builds and WBPP runs across databases, running and queued | |
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

Jobs return at once. Poll `get_jobs` for progress.

The server's instructions tell the agent the ground rules PSF Guard keeps
for people: `analyze_sequence` and `get_image_quality` suggest, and nothing
changes until `grade_images` runs; catalog predictions and header values are
not pixel evidence.

## Looking into a stack

A typical question is "where does this pattern in the SII stack come from?"
`list_stacks` names the channel's `job_id`. `get_stack_image` with
`stretch: "background"` shows the pattern; a crop shows it at full detail.
`get_stack_calibration` shows whether some sessions had no flat or a dark
master built from two frames, and `explain_calibration` on one of their
lights says why: a flat set marked for later lights, flats at another
rotator angle, darks of another exposure. `get_stack` shows, per night,
whether the frames were dithered: frames that walk one way without dithers
turn anything fixed to the sensor into streaks.

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
  a POST with 405 and a note naming the endpoint, so a client given the
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
