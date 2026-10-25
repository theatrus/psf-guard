# Remote workers

Status: **built for RC-Astro steps.** Stack builds are next.
Last updated: 2026-10-24

This record describes how PSF Guard hands work to another machine: the task
board, the worker protocol, how files move, and which work goes first. It is
step 11 of [One job line](job-line.md#migration). The user guide is
[Workers](../REMOTE_WORKERS.md).

## Shape

```text
work on this server ──submit──► task board ◄──claim / report / complete── worker
   (waits on a Ticket)           (server)                                  │
                                    ▲                        this server's worker (in process)
                                    └──────── the same calls ────────────┘
```

- **The task board** (`src/server/workers/board.rs`) holds the tasks waiting
  for a worker, the workers connected, and the lease each running task is
  under. It lives on `AppState.workers`.
- **A task** is a kind, a JSON spec, named input files, named output files
  and what a worker must offer to run it (`TaskNeeds`). The shared types are
  in `src/remote_work/`.
- **A worker advertises its work**: the task kinds it takes (`Offers.work`)
  and, for RC-Astro, the tools it takes. `psf-guard worker --work` narrows
  them; without it a worker takes everything it can run.
- **Work on this server** submits a task and waits on its `Ticket`, on its
  own thread. Dropping the ticket takes the task off the board and removes
  its files.
- **A worker** claims the first queued task it can run, reports progress,
  sends back its outputs, and completes or fails the task.
- **This server's own worker** (`src/server/workers/local.rs`) registers on
  the board and claims tasks through the same calls, without HTTP. Every
  RC-Astro step this server runs takes the path a remote one does, as
  [the job line decided](job-line.md#decisions-on-earlier-open-questions).

The board sits below the job line: a processing run is still an entry in the
stack line, and only its RC-Astro steps become tasks. Moving whole line
entries to workers waits for the work below.

## Leases and priority

- A claim gives the worker a lease that lasts a minute. Each progress report
  renews it; the worker reports six times a lease.
- A lease that runs out puts the task back in its place, with one more
  attempt counted. A task that loses three leases fails.
- A worker runs one task at a time. A claim while it holds a task gives that
  task up. This server's own worker does too, so two processing runs no
  longer run RC-Astro tools on this server at once; one GPU runs them
  faster one after the other.
- A person's task goes ahead of automatic work. Otherwise tasks run in the
  order they were submitted.
- This server's worker is the fallback. It claims a task only when no
  connected worker of another machine can run it, so a worker that takes a
  tool runs it even while busy, and this server takes over once that worker
  has not been heard from for 90 seconds.
- A queued task that no connected worker can run waits 30 seconds for one,
  then fails with what it needs.
- A stop takes the task off the board. The worker hears at its next report
  that the server no longer wants it, and stops. A task this server's worker
  runs is stopped before the stop returns, so a stopped tool is killed and
  its folder removed, as before.
- This server's worker holds no expiring lease: it cannot vanish without the
  server, and a tool may report nothing for minutes.

## Trust

A worker gets a spec and the files that spec names, and nothing else.

- **Tokens.** Settings › Workers mints one token per worker (`psfw_` and 64
  hex digits). The registry keeps its SHA-256 (`remote_workers.tokens`).
  The workers' routes (`/api/workers/v1/`) skip the user sign-in, and each
  request is checked against the tokens. A token reaches only the sessions
  registered with it and the tasks those sessions hold. Revoking a token
  removes its sessions and requeues their tasks.
- **Leases.** Every request about a task sends its lease
  (`x-psf-worker-lease`). An answer under a lease that is not current is
  refused with `409`.
- **Outputs.** An upload carries its SHA-256 (`x-psf-sha256`) and is
  refused when the bytes differ. On completion the server reads each output
  in the task folder and checks its size and digest against the worker's
  list, and checks that every required output is there. Only then does the
  waiting work read it. File names are restricted, so no name leaves the
  task's folder.
- **Settings.** Minting, revoking and the exchange folder need read-write
  access on a server that allows database management. A token is minted
  only from a signed-in session, not with an API token.

## Files

A task's files live in its own folder in the exchange folder
(`<exchange>/tasks/<task>/`). The exchange folder is `<cache>/workers`
unless Settings › Workers names another; a server start clears what an
earlier run left there.

- **Remote mode.** The worker downloads each input
  (`GET …/inputs/{name}`) into its scratch folder, and uploads each output
  (`PUT …/outputs/{name}`), streamed in both directions.
- **Shared mode.** The worker registers its maps from server folders to its
  own. The claim sends each input's server path and size; the worker reads
  it through its maps when the size matches and downloads it otherwise. The
  server writes a probe file in the exchange folder at registration; a
  worker that reads it and writes the same bytes beside it is sent output
  paths too, and writes its outputs straight into the task folder. A worker
  that fails the probe uploads its outputs.

Inputs that are already on shared storage, such as a stack build's frames,
will reach a shared worker by path without passing through the exchange
folder.

## Protocol

All under `/api/workers/v1`, with `Authorization: Bearer psfw_…`:

| Request | Answer |
|---|---|
| `POST /register` with name, version, protocol, offers and file access | session, lease and poll seconds, and a probe for shared mode |
| `POST /sessions/{s}/probe` with the probe's digest | whether files go by path |
| `POST /sessions/{s}/claim` | `200` with a task, or `204` after 25 seconds without one |
| `POST /sessions/{s}/tasks/{t}/progress` with a fraction | whether to stop |
| `GET /sessions/{s}/tasks/{t}/inputs/{name}` | the file |
| `PUT /sessions/{s}/tasks/{t}/outputs/{name}` with its digest | `204` |
| `POST /sessions/{s}/tasks/{t}/complete` with the result and each output's size and digest | `204`, or why the check failed |
| `POST /sessions/{s}/tasks/{t}/fail` with the error | `204` |
| `POST /sessions/{s}/leave` | `204`; its task goes back in the queue |

`401` means the token is not valid, and the worker command stops. `404`
means the session is unknown, such as after a server restart, and the worker
registers again. A worker that registers again under its name replaces its
old session. The protocol has a version; a server refuses a worker that
speaks another one and explains which release to install.

## RC-Astro steps

Each tool in a chain is one task (`rc_astro_step`). The chain's prefix
caching stays on the server, so a chain resumes from its deepest cached
step whichever worker ran the earlier steps.

- **Schemas.** A chain's schemas come from the connected workers by name
  first, then from this server's rc-astro, licensed first. A step falls
  back to this server only at the same version, since the version is part
  of the chain's cache identity; otherwise it fails, and the next apply picks
  this server's version. This server's
  rc-astro is probed again once a minute, so installing it needs no
  restart. `PSF_GUARD_RC_ASTRO=off` leaves rc-astro to the workers.
- **Needs.** The task needs that tool at that CLI and model version,
  licensed, since both versions are part of the result's cache identity.
- **Spec.** The schema, the request and the headers rc-astro reads, with
  the padded image as `input.fits`. Outputs are `processed.fits` and,
  optionally, `stars.fits`. Coverage is restored on the server.
- **Progress.** The stage reads `NoiseXTerminator on studio-pc` while a
  remote worker runs it.

## Next: stack builds

A stack build still reads this server's catalog while it runs: it chooses
the reference frame from the WCS and score caches, builds calibration
masters from the calibration catalog, and records each channel's progress
on the build record. Before a build can be a task:

1. Choose the reference, the masters and the orientation anchors while the
   build is prepared at its turn, and put them in the spec.
2. Report a channel's progress through the task, and write the build record
   from what the task reports.
3. Make the stacked FITS, the frame decisions and the checkpoint the task's
   outputs, published on this server as now.
4. Send frames and masters by path to a shared worker, and through the
   server otherwise. A stopped build's checkpoint is an output, so the build
   can resume on another worker or on this server.

A build would then need cores, memory and optionally a GPU rather than a
tool. Assignment can weigh the bytes to send against waiting for this
server.
