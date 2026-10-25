# Workers

A worker is another computer that runs work for a PSF Guard server. Use one
when the server lacks something the work needs: an RC-Astro licence, a GPU,
or more processor cores. The server keeps the catalog and every result.

Workers run two kinds of work:

- **RC-Astro tools**: BlurXTerminator, NoiseXTerminator and StarXTerminator,
  for stack previews, color previews and narrowband stars.
- **Stack builds**: each channel of a build, stacked from its frames. Stack
  builds stay on the server unless you send them to workers; see [Stack
  builds on a worker](#stack-builds-on-a-worker).

WBPP runs stay on the server.

## Add a worker

You need an editor account on a server that allows database management.

1. Install PSF Guard on the worker computer, the same release as the server.
2. Install rc-astro on the worker and license the tools there. Run
   `rc-astro download-models` once, so the first run does not wait for the
   models to download.
3. On the server, open **Settings › Workers** and click **+ Add a worker**.
   Enter a name for the worker, such as `studio-pc`, and click **Create
   token**.
4. Copy a command PSF Guard shows: one for macOS and Linux shells, one for
   Windows PowerShell. The token in it is shown only once. When the page's
   address only works on the server itself, such as `localhost`, the command
   contains `THIS-SERVER:PORT`; replace it with the server's address on your
   network.
5. Run the command on the worker:

   ```bash
   PSF_GUARD_WORKER_TOKEN=psfw_… psf-guard worker --server http://192.168.1.10:3000
   ```

The worker lists the tools it found and joins the server. **Settings ›
Workers** then shows it as connected, with its tools and the tasks it is
running. Leave the command running. Stop it with Ctrl-C: the worker stops
the tool it is running, and the server queues the task again.

`--token psfw_…` also works, but the variable keeps the token out of the
process list. `--name` changes the name the server shows, which is the host
name by default. `--rc-astro PATH` gives the rc-astro executable when it is
not on `PATH`. `--slots N` lets the worker run up to N tasks at once, from 1
to 16; the default is 1, since each RC-Astro step uses the GPU.

## Choose the work a worker accepts

A worker accepts everything it can run unless you list its work with
`--work`. Repeat it for more:

- `--work stack` accepts stack builds.
- `--work rc-astro` accepts every RC-Astro tool installed on the worker.
- `--work bxt`, `--work nxt` or `--work sxt` accepts only that tool.

For example, `--work bxt --work sxt` sends BlurXTerminator and
StarXTerminator to a GPU computer, and leaves NoiseXTerminator and stack
builds to the server. **Settings › Workers** lists the tools each worker
accepts, and whether it accepts stack builds.

## Choose where work runs

Under **Where work runs** in **Settings › Workers**, choose where RC-Astro
tools run, and where stack builds run:

- **Workers first.** A connected worker that accepts the work runs it. The
  server runs the work itself only while no such worker is connected, so it
  is the fallback when a worker is stopped or unreachable. This is the
  default for RC-Astro tools.
- **Workers only.** Only workers run the work. With no worker connected, it
  fails rather than run on the server.
- **This server only.** The server runs the work, and workers are not sent
  it. This is the default for stack builds.

WBPP runs always run on the server.

## Stack builds on a worker

To stack builds on another computer, such as one with more cores or a GPU:

1. Start the worker with `--work stack`, or with no `--work` at all. Use
   shared files when you can: a worker in remote mode downloads every frame
   of a channel before it stacks it, about 50 to 120 MB a frame.
2. In **Settings › Workers**, under **Where work runs** › **Stack builds**,
   choose **Workers first** or **Workers only**.

The server sends a channel only to a worker on its own PSF Guard release
and Seiza stacking version, with the frames, masters, checkpoint and
settings it would use itself. **Settings › Stacking › GPU** applies on the
worker too: with **Use the GPU** off, the worker stacks on its CPU even if
it has a GPU. A GPU's passes can differ slightly from a CPU's, so a channel
stacked on a worker with a GPU can differ slightly from one this server
stacked on its CPU.

The server still does the work that reads its catalog: it chooses each
channel's reference frame, builds the calibration masters, decides whether
the build continues a checkpoint, and plate-solves the reference for a
north-up stack. The worker stacks the frames with those masters, runs the
final pass and writes the stack. The server then stores the stack and its
checkpoint and renders the previews.

While a worker stacks a channel:

- The card fills in frame by frame, a few seconds behind the worker, and
  shows **Stacked on** with the worker's name.
- The card's **Live stack** preview shows the worker's running image. The
  worker renders it about every five seconds and sends the newest one after
  the frames it includes. The server writes each image over the last as a
  temporary file, and the finished preview replaces it. An image lost while
  the server is unreachable is not sent again; the next one replaces it.
- The server's cores are free for its other work.
- A worker running several tasks (`--slots`) gives each channel its share
  of its cores and memory. The final pass keeps its copies of the frames in
  the worker's `--work-dir`.
- **Stop**, or a person's build that stops an automatic one, asks the worker
  to finish with the frames it has. It saves a checkpoint and sends it, and
  the next build continues from it on any worker or on the server. The
  server waits as long as the worker keeps reporting; a worker that stops
  answering for two minutes loses what it had.
- Under **Workers first**, the server stacks the channel itself when no
  worker can take it or a worker fails it.
- A mosaic always stacks on the server.

## Choose how files reach the worker

Each RC-Astro step reads one image and writes one or two. Those files go
through the server's *exchange folder*. PSF Guard keeps them in a
`psf-guard-exchange` folder inside it and removes only what it wrote there.

- **Through the server** (the default, `--files remote`). The worker
  downloads each input from the server and uploads each result. It needs only
  the server's address. A 6000 × 4000 color image is about 290 MB each way.
- **Shared files** (`--files shared`). The worker reads and writes the files
  in the exchange folder itself, on storage it shares with the server, such
  as an NFS or SMB share. No image crosses the network twice.

For shared files, keep the exchange folder on the shared storage:

1. In **Settings › Workers**, under **Exchange folder**, enter a folder on
   the share as the server sees it, such as `/srv/astro`, and click **Save
   folder**.
2. Start the worker with a map from the server's path to the worker's:

   ```bash
   psf-guard worker --server http://nas:3000 \
     --map /srv/astro=/Volumes/astro
   ```

   Repeat `--map` for more folders. A map implies `--files shared`. With
   `--files shared` and no map, the worker uses the server's paths as they
   are.

When the worker joins, the server writes a test file in the exchange folder.
The worker reads it and writes a reply beside it. When both work, the worker
logs `Shared files` and **Settings › Workers** shows **Shared files**.
Otherwise the worker logs the reason, and its files go through the server.
A file the worker cannot see, or whose size does not match, also goes
through the server.

## Which worker runs a tool

When you apply processing, PSF Guard picks each tool's version from the
connected workers by name first, then from the server's rc-astro. Licensed
tools win over unlicensed ones. The step then goes to the first worker with
a free slot and that exact tool, CLI and model version, because those
versions are part of the result's cache identity.

- The processing panel shows where each tool runs, such as
  **BlurXTerminator on studio-pc · NoiseXTerminator on this server**.
- A worker that is busy keeps its queued steps: under **Workers first**, the
  server does not take them while that worker is connected.
- A step falls back to the server only when the server has the same rc-astro
  version as the worker. Otherwise the step fails, and the next **Apply
  processing** uses the server's version. Install the same rc-astro release
  everywhere to avoid this.
- While a step runs on a worker, the progress label shows the worker's
  name, such as **NoiseXTerminator on studio-pc**.
- The server's own worker runs one step at a time. A person's work goes
  ahead of automatic re-applies.
- **Settings › Workers** lists tasks waiting for a worker and what each
  needs.

When no connected worker offers a tool and the server has none, applying
processing explains that rc-astro is not installed on this server or on a
connected worker. A step that is queued when its worker disconnects goes to
the server when it can run it there. Otherwise it waits 30 seconds for a
worker to connect, then fails with **No connected worker offers …**. A
worker counts as disconnected 90 seconds after the server last heard from
it.

After a server restart, workers take a little while to join again.
Processing that was running before the restart waits up to 30 minutes for
its workers to return. A worker reads its rc-astro tools when it starts, so
restart it after you license or upgrade rc-astro there.

## Stop, failures and recovery

- **Stop** on a processing run takes its task off the board. A tool on the
  server is killed at once. A worker stops its tool at its next progress
  report, within about ten seconds.
- A worker reports at least every ten seconds while it downloads, runs and
  uploads a task. When it stops answering for a minute, the task goes back in
  the queue.
- A task also goes back in the queue when its worker disconnects, restarts
  or has its token revoked. A task that goes back three times fails, with
  the reason the last time.
- When a step fails on a worker, such as after its GPU driver crashed, the
  server runs the step itself when it has the same tool version, and that
  worker is not sent the step again. Otherwise the run fails with the
  worker's message and its name.
- The server checks each result's size and SHA-256 digest before it uses
  the result. A result sent under an old lease is refused, and each lease
  writes its results in a folder of its own.

## Revoke a worker

In **Settings › Workers**, click **Revoke** beside the worker's token. The
server forgets the worker at once and queues its tasks again. At its next
progress report or claim, the worker stops its tool and exits with a message
to create a new token.

## Security

A worker token can register a worker, claim tasks and read the files of the
tasks it claims. It cannot sign in, read the catalog or reach any other API.
The server stores only the token's SHA-256 hash.

- In shared mode, a worker can read the whole exchange folder, including
  other tasks' files, through the share. Share only that folder with
  workers you trust as much as the server.
- A worker refuses a task whose id or file names would reach outside its
  own work folder.
- Use HTTPS, or a trusted network, between the worker and the server: the
  token and the task files travel in each request.

See [Remote workers](design/remote-workers.md) for the protocol.
