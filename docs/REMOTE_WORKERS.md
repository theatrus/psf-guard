# Workers

A worker is another computer that runs work for a PSF Guard server. Use one
when the server lacks something the work needs: an RC-Astro licence, a GPU,
or more processor cores. The server keeps the catalog and every result. A
worker receives only the files of the task it runs.

Workers run RC-Astro tools today: BlurXTerminator, NoiseXTerminator and
StarXTerminator, for stack previews, color previews and narrowband stars.
Stack builds are planned next.

## Add a worker

You need an editor account on a server that allows database management.

1. Install PSF Guard on the worker computer, the same release as the server.
2. Install rc-astro on the worker and license the tools there. Run
   `rc-astro download-models` once, so the first run does not wait for the
   models to download.
3. On the server, open **Settings › Workers** and click **+ Add a worker**.
   Enter a name for the worker, such as `studio-pc`, and click **Create
   token**.
4. Copy the command PSF Guard shows. The token in it is shown only once.
5. Run the command on the worker:

   ```bash
   PSF_GUARD_WORKER_TOKEN=psfw_… psf-guard worker --server http://192.168.1.10:3000
   ```

The worker lists the tools it found and joins the server. **Settings ›
Workers** then shows it as connected, with its tools and the task it is
running. Leave the command running. Stop it with Ctrl-C; the server queues
its task again for another worker.

`--token psfw_…` also works, but the variable keeps the token out of the
process list. `--name` changes the name the server shows, which is the host
name by default. `--rc-astro PATH` gives the rc-astro executable when it is
not on `PATH`.

## Choose the work a worker takes

A worker takes everything it can run unless you list its work with
`--work`. Repeat it for more:

- `--work rc-astro` takes every RC-Astro tool installed on the worker.
- `--work bxt`, `--work nxt` or `--work sxt` takes that tool only.

For example, `--work bxt --work sxt` sends BlurXTerminator and
StarXTerminator to a GPU computer, and leaves NoiseXTerminator to this
server. **Settings › Workers** lists the tools each worker takes.

## Choose how files reach the worker

Each RC-Astro step reads one image and writes one or two. Those files go
through the server's *exchange folder*.

- **Through the server** (the default, `--files remote`). The worker
  downloads each input from the server and uploads each result. It needs only
  the server's address. A 6000 × 4000 color image is about 290 MB each way.
- **Shared files** (`--files shared`). The worker reads and writes the files
  in the exchange folder itself, on storage it shares with the server, such
  as an NFS or SMB share. No image crosses the network twice.

For shared files, keep the exchange folder on the shared storage:

1. In **Settings › Workers**, under **Exchange folder**, enter a folder on
   the share as the server sees it, such as `/srv/astro/psf-exchange`, and
   click **Save folder**.
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

A connected worker that takes a tool runs it ahead of this server. This
server runs the tool itself only when no connected worker takes it, so it is
the fallback when a worker is stopped or unreachable.

When you apply processing, PSF Guard picks each tool's version from the
connected workers by name first, then from this server's rc-astro. Licensed
tools win over unlicensed ones. The run then goes to the first free worker
with that exact tool, CLI and model version, because those versions are part
of the result's cache identity.

- The processing panel shows where the tools run: **Runs on the worker
  studio-pc, or on this server while no worker is connected**.
- A worker that is busy keeps its queued steps: this server does not take
  them while the worker is connected.
- A step falls back to this server only when this server has the same
  rc-astro version as the worker. Otherwise the step fails; the next
  **Apply processing** uses this server's version. Install the same
  rc-astro release everywhere to avoid this.
- While a step runs elsewhere, its progress names the worker, such as
  **NoiseXTerminator on studio-pc**.
- Each worker, this server included, runs one step at a time. A person's
  work goes ahead of automatic re-applies.
- **Settings › Workers** lists tasks waiting for a worker and what each
  needs.

When no connected worker offers a tool and this server has none, applying
processing explains that rc-astro is not installed on this server or on a
connected worker. A step that is queued when its worker disconnects goes to
this server when it can run it. Otherwise it waits 30 seconds for a worker
to connect, then fails with **No connected worker offers …**. A worker
counts as disconnected 90 seconds after the server last heard from it.

To keep this server from running rc-astro itself, so that only workers run
it, start the server with `PSF_GUARD_RC_ASTRO=off`.

## Stop, failures and recovery

- **Stop** on a processing run takes its task off the board. A tool on this
  server is killed at once. A worker stops its tool at its next progress
  report, within about ten seconds.
- A worker reports at least every ten seconds. When it stops answering for a
  minute, the task goes back in the queue. A task that loses three workers
  this way fails.
- An error on the worker, such as an unlicensed tool, fails the run with the
  worker's message and its name.
- The server checks each result's size and SHA-256 digest before it uses
  the result. A result sent under an old lease is refused.

## Revoke a worker

In **Settings › Workers**, click **Revoke** beside the worker's token. The
worker disconnects at once, its task goes back in the queue, and the worker
command stops with a message to create a new token.

## Security

A worker token can register a worker, claim tasks and read the files of the
tasks it claims. It cannot sign in, read the catalog or reach any other API.
The server stores only the token's SHA-256 hash. Use HTTPS, or a trusted
network, between the worker and the server: the token and the task files
travel in each request.

See [Remote workers](design/remote-workers.md) for the protocol.
