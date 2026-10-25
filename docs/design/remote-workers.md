# Remote workers

Status: **built for RC-Astro steps and stack builds' channels.**
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
- **A worker advertises its work**: the task kinds it accepts
  (`Offers.work`) and, for RC-Astro, the tools it accepts. `psf-guard worker
  --work` narrows them; without it a worker accepts everything it can run.
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
  renews it; the worker reports six times a lease, through its downloads,
  the run and its uploads.
- A task goes back in its place when its lease runs out, or when its worker
  leaves, registers again or has its token revoked. Each counts one attempt;
  the third fails the task with the reason.
- A worker runs as many tasks at once as its slots (`--slots`, 1 to 16,
  default 1). This server's own worker has one slot, so two processing runs
  no longer run RC-Astro tools on this server at once; one GPU runs them
  faster one after the other.
- A person's task goes ahead of automatic work. Otherwise tasks run in the
  order they were submitted.
- **Placement.** Each kind of task has a `WorkPlacement`, set in Settings ›
  Workers and stored in `remote_workers` in the registry: `workers_first`
  (the default), `workers_only` or `this_server`. Under workers first, this
  server's worker is the fallback: it claims a task only when no connected
  worker of another machine can run it, so a worker that accepts a tool runs
  it even while busy, and this server takes over once that worker has not
  been heard from for 90 seconds. Stack builds and WBPP runs are not tasks
  yet and stay on this server.
- This server's worker always counts as connected, since a tool it runs may
  report nothing for minutes.
- A queued task that no connected worker can run waits 30 seconds for one,
  then fails with what it needs.
- A stop takes the task off the board. The worker's next report is
  answered with a stop, and the worker stops the task.
- A task that a worker of another machine fails goes back in the queue for
  this server when this server can run it at the same version, such as
  when the worker's GPU driver or licence broke. That worker is not sent the
  task again. Otherwise the task fails with the worker's error. A task this server's worker
  runs is stopped before the stop returns, so a stopped tool is killed and
  its folder removed, as before.
- This server's worker holds no expiring lease: it cannot vanish without the
  server, and a tool may report nothing for minutes.

## Trust

A worker in remote mode gets a spec and the files that spec lists, and
nothing else. A worker in shared mode can also read the rest of the
exchange folder through the share.

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

A task's files live in its own folder,
`<exchange>/psf-guard-exchange/tasks/<task>/`, and each lease writes its
outputs in `<task>/<lease>/`, so a worker that lost its lease cannot
overwrite the next worker's outputs. The exchange folder is
`<cache>/workers` unless one is chosen in Settings › Workers. PSF Guard
removes only what it wrote: a server start removes task folders and probes
older than six hours, named by a task or session id, since another server
may share the folder.

- **Remote mode.** The worker downloads each input
  (`GET …/inputs/{name}`) into its scratch folder, and uploads each output
  (`PUT …/outputs/{name}`), streamed in both directions.
- **Shared mode.** The worker registers its maps from server folders to its
  own. The claim sends each input's server path and size; the worker reads
  it through its maps when the size matches and downloads it otherwise. The
  server writes a probe file in the exchange folder at registration; a
  worker that reads it and writes the same bytes beside it is sent output
  paths too, and writes its outputs straight into its lease's folder. A worker
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
| `PUT /sessions/{s}/tasks/{t}/preview` with a PNG and its frame count in `x-psf-frames` | `204` |
| `PUT /sessions/{s}/tasks/{t}/outputs/{name}` with its digest | `204` |
| `POST /sessions/{s}/tasks/{t}/complete` with the result and each output's size and digest | `204`, or why the check failed |
| `POST /sessions/{s}/tasks/{t}/fail` with the error | `204` |
| `POST /sessions/{s}/leave` | `204`; its task goes back in the queue |

`401` means the token is not valid: the worker stops its tasks and exits.
`404` means the session is unknown, such as after a server restart: the
worker stops its tasks and registers again. Either answer to a progress
report stops the task in hand. The worker refuses an assignment whose task
id is not a UUID or whose file names are not plain names. A worker that registers again under its name replaces its
old session. The protocol has a version; a server refuses a worker that
speaks another one and explains which release to install.

## RC-Astro steps

Each tool in a chain is one task (`rc_astro_step`). The chain's prefix
caching stays on the server, so a chain resumes from its deepest cached
step whichever worker ran the earlier steps.

- **Schemas.** A chain's schemas come from the connected workers that the
  placement allows, by name first, then from this server's rc-astro,
  licensed first. A processing run restored from the journal retries, for
  up to 30 minutes, while no connected worker offers its tools, since
  workers join again some time after a restart. A step falls
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

## Stack channels

Each channel of a stack build can be a task (`stack_channel`). The build's
own work splits where the catalog stops:

- **This server prepares the channel** in `mono_build::run_group`, as it
  does for itself: the reference (`choose_references`), the calibration plan
  and its masters, the resume decision, and the orientation, including the
  reference's plate solve for a north-up stack.
- **The integration** (`stack_preview::integrate`) is the same code on
  either machine: it opens or resumes the live stack, swaps each session's
  masters, pushes the frames, checkpoints, measures the SNR curve, runs the
  final pass, orients and writes the FITS. It reads neither the catalog nor
  the build record, and reports through `ChannelEvents`.
- **On a worker** (`stack_preview::remote_channel`), the spec carries the
  group with its frames named by input, each session's masters as a
  `MasterRecipe` (the bias, dark and flat files and how they combine, or a
  flat with a fitted pedestal), the checkpoint's ledger, the method, the GPU
  setting and the orientation. The worker loads the masters from their
  files, so they match this server's, and reopens the checkpoint context.
- **Events.** Each `ChannelEvents` call becomes an event in a progress
  report, numbered over the task; the server skips numbers it already has,
  and the worker keeps each event until the server acknowledges it. The
  worker reports every two seconds while it runs, and sends every event
  before it completes, so the card is filled before the result arrives.
- **Running image.** The worker renders the running image on this
  server's cadence (`live::Cadence`, about every five seconds, with batches
  sized to keep it). It sends the newest PNG once the server has
  acknowledged every event reported before the image was rendered, so the
  card never shows an image ahead of its frames; the last one goes after
  the final events. The board keeps only the newest per task, in memory,
  cleared on requeue; the waiting build reads it after the events and
  writes it where this server writes its own running image. A restarted
  stack (a requeue or a fallback) clears the card's image. A failed send is
  not retried, and the image does not renew the lease.
- **Outputs.** `stack.fits`, `clipped.fits`, and the checkpoint's context
  and manifest. The server moves them into place and renders the previews.
  The frame decisions keep this server's file fingerprints; the worker
  checks its own copies (`PreparedFrame::read_fingerprint`) in the final
  pass.
- **Needs.** A worker that takes stack builds on this server's PSF Guard
  release and Seiza stacking output version: the method and the final pass
  are PSF Guard's, and the version is part of a stack's identity.
- **Resources.** The worker gives a channel its share of its cores and
  free memory, one per slot, and keeps the final pass's copies of the frames
  in its work folder. This server's GPU setting applies on the worker.
- **Placement.** `remote_workers.stack` in the registry, this server only by
  default. This server stacks its own channels in process rather than
  through its worker; under workers first, it stacks the channel when no
  worker can take it. It releases its
  CPU lease while a worker stacks, and leases again for the previews.
- **Stop.** A stop on the build asks the worker to finish
  (`ProgressReply::finish`): the integration stops at its next frame and
  checkpoints, and the worker completes the task with the checkpoint, so the
  next build continues from it. The server waits until two minutes after
  the worker's last report or upload. A worker asked to finish before it
  began drops the task, and the server keeps its checkpoint. A stopped task
  goes to no new worker, and a requeued task's events start again from 0.
- **Failure.** Under workers first, a channel a worker fails is stacked on
  this server.
- **Mosaics** go to workers too. The spec carries each frame's panel
  target and the group's panels; the worker plans the canvas in its work
  folder and publishes on it (`Orientation::MosaicCanvas`). Planning, like
  the final pass, checks the copy it reads (`read_fingerprint`). A mosaic
  writes no checkpoint, and this server keeps its own. A test stacks a
  synthetic four-panel mosaic here and on a worker, in both file modes,
  and checks the pixels match bit for bit.

A test stacks one synthetic channel on this server and on a real worker,
over HTTP in both file modes, and checks the pixels match bit for bit on
one machine. A worker's GPU can make its passes differ slightly from a
CPU's, as on this server.
