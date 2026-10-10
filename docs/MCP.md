# MCP server for agents

A running PSF Guard server is also a Model Context Protocol (MCP) server.
An agent such as Claude Code can list catalogs, read grades and quality
evidence, score sequences, apply grades, and start imports, quality scans
and WBPP runs through it. The endpoint is `/api/mcp` on the same port as
the UI. It uses the streamable HTTP transport, stateless, with JSON
answers, so a reverse proxy needs nothing extra.

## Connect

On a server with user accounts, the endpoint takes a personal API token.
Mint one under **Settings → Users → API tokens**, or from the CLI:

```bash
psf-guard users token create editor --label "claude on laptop" --expires-days 90
```

The token is shown once. Send it as a bearer credential. For Claude Code:

```bash
claude mcp add --transport http psf-guard https://guard.example/api/mcp \
  --header "Authorization: Bearer psfg_…"
```

Any MCP client that speaks streamable HTTP works the same way: URL
`https://guard.example/api/mcp`, header `Authorization: Bearer psfg_…`.

A loopback server with no accounts, which is what the desktop app and
`psf-guard server` on a developer machine run, needs no token:

```bash
claude mcp add --transport http psf-guard http://127.0.0.1:3000/api/mcp
```

A network server with no accounts answers 401 here as it does everywhere
else. See [server authentication](AUTHENTICATION.md).

## What a token may do

A token acts as its user. An editor's token can grade and start jobs; a
viewer's token, or any token minted **read only**, can only read. The tools
that change the catalog check this themselves and answer with a tool error
rather than a protocol failure, so an agent can explain the refusal.
`start_wbpp_run` and `cancel_wbpp_run` further need a server started with
`--allow-database-management`, like the UI action.

A token cannot mint or revoke tokens. That takes a browser session.

## Tools

Every tool but `list_databases` takes `database`, the id (slug) or name
from `list_databases`. Results are JSON text, the same shapes the UI API
returns.

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
