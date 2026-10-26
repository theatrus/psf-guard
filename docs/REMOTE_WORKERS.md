# Workers

A worker is a PSF Guard process that runs processing tasks for a server.
Run a worker on another computer to use its RC-Astro licenses, GPU or
processor cores. The server stores the catalog and processing results.

Workers run two kinds of work:

- **RC-Astro tools**: BlurXTerminator, NoiseXTerminator and StarXTerminator,
  for stack previews, color previews and narrowband stars.
- **Stack builds**: each channel of a build, stacked from its frames. Stack
  builds stay on the server unless you send them to workers; see [Stack
  builds on a worker](#stack-builds-on-a-worker).

WBPP runs stay on the server.

## Add a worker

To create a worker token, you need an editor account on a server that allows
database management.

1. Install PSF Guard on the worker computer, the same release as the server.
2. If the worker will run RC-Astro tools, install rc-astro and license the
   tools on that computer. Run `rc-astro download-models` to download the
   models before the first task. Workers that only build stacks do not
   need rc-astro.
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

## Run a worker from the desktop app

On macOS and Windows, you can configure a worker in the PSF Guard app.
After you start it, the worker runs in the menu bar or system tray and
continues running when you close the app window.

1. Install the same PSF Guard release on the worker computer and the server.
   To run RC-Astro tools, install rc-astro and license the tools on the
   worker computer too.
2. On the server, open **Settings › Workers**, click **+ Add a worker**, and
   create a token. Copy the token; PSF Guard shows it only once.
3. On the worker computer, open PSF Guard and choose **Settings › Workers ›
   Worker on this computer**.
4. Enter the **Server address**, such as `http://192.168.1.10:3000`, and the
   **Worker token**. Leave **Worker name** blank to use the computer's host
   name. Under **Task types**, select **Stack builds**, **RC-Astro tools**,
   or both.
5. Set **Concurrent tasks** to the maximum number of tasks this worker can
   run at the same time, from 1 to 16. Start with 1 for GPU work.
6. To start the worker when you log in, select **Start worker at login**.
7. Click **Save worker settings**, then **Start worker**.

**Worker on this computer** shows the connection status and running tasks.
The controls affect the worker on this computer:

- **Pause worker** stops new task requests and lets running tasks finish.
  A request already sent to the server may still return one task.
  **Resume worker** allows new task requests again.
- **Stop worker** stops the worker and returns unfinished tasks to the
  server's queue.
- After changing settings, click **Save worker settings**, then **Restart
  worker** to apply them to a running worker. Leave **Worker token** blank
  to keep the saved token.

The menu bar or system tray shows the same status. It provides **Pause
worker** or **Resume worker**, **Open PSF Guard**, and **Quit worker**.
**Quit worker** stops the worker and closes its menu bar or tray icon.

- Task files transfer through the server by default. To use shared storage,
  open **Task files** and select **Use shared storage**; see [Choose how
  files reach the worker](#choose-how-files-reach-the-worker).
- Leave **Temporary task folder** blank to use the system temporary folder.
  This folder stores task files while tasks run.
- PSF Guard saves worker settings and the token in `worker.json` in its
  settings folder. On macOS and Linux, only your user can read this file.
  `psf-guard worker --config FILE` uses the same settings without a menu bar
  or tray icon.
- **Start worker at login** creates a LaunchAgent on macOS at
  `~/Library/LaunchAgents/com.theatrus.psf-guard.worker.plist`, or a
  **PSF Guard Worker** entry in the Windows registry's `Run` key.
  To remove the login entry, turn off **Start worker at login** and click
  **Save worker settings**.
- Click **Open logs** to open the app's log folder. The worker's log
  filenames start with `worker`.

On Linux, follow [Run a worker as a Linux
service](#run-a-worker-as-a-linux-service).

## Run a worker as a Linux service

The Fedora RPM and the Debian package install a `psf-guard-worker` systemd
service. It is not enabled, so it runs only after you set it up.

1. On the server, open **Settings › Workers**, click **+ Add a worker**,
   and create a token.
2. On the worker, edit `/etc/psf-guard/worker.conf` as root. The Debian
   package installs an example instead; copy it first and keep it readable
   by root only, since it holds the token:

   ```bash
   sudo install -D -m 600 /usr/share/doc/psf-guard/worker.conf.example /etc/psf-guard/worker.conf
   sudoedit /etc/psf-guard/worker.conf
   ```

3. Set `PSF_GUARD_WORKER_SERVER` to the server's address and
   `PSF_GUARD_WORKER_TOKEN` to the token. Put other options for `psf-guard
   worker`, such as `--work stack` or `--slots 2`, in
   `PSF_GUARD_WORKER_EXTRA_ARGS`.
4. Start the worker, and start it at boot:

   ```bash
   sudo systemctl enable --now psf-guard-worker
   journalctl -u psf-guard-worker -f
   ```

The service runs as a user of its own, with its files in
`/var/lib/psf-guard-worker`. `systemctl stop psf-guard-worker` stops the
worker, and the server queues its running task again. While the server
cannot be reached, the worker keeps trying. When the server refuses the
token, or the token is missing, the worker stops, and the service starts it
again every 30 seconds; `journalctl -u psf-guard-worker` shows why.

- **GPU stacking.** The service joins the `render` and `video` groups, so
  stacking can use an AMD or Intel GPU through Vulkan. NVIDIA GPUs need no
  group.
- **rc-astro with an NVIDIA GPU.** On a computer with no desktop session,
  load the `nvidia-uvm` module at boot, since the service cannot load it:
  add `nvidia-uvm` to a file in `/etc/modules-load.d/`.
- **Shared storage, or rc-astro licensed by a user.** Run the service as a
  user that can read and write the shared folders, or the user who
  licensed rc-astro. Run `sudo systemctl edit psf-guard-worker` and add the
  user and each shared folder:

  ```ini
  [Service]
  DynamicUser=no
  User=astro
  ReadWritePaths=/mnt/astro
  ```

`PSF_GUARD_WORKER_EXTRA_ARGS` is split on spaces, so it cannot hold a path
that contains one.

## Run a worker as a Windows service

Use a Windows service when the worker needs to run before anyone logs in.
The service starts with Windows. It uses its own copies of the app and
settings, separate from the desktop worker.

1. Open **Settings › Workers › Worker on this computer** in the desktop
   app and enter the worker settings. Turn off **Start worker at login**,
   then click **Save worker settings**. If the desktop worker is running,
   click **Stop worker** before installing the service.
2. Open PowerShell as administrator and run:

   ```powershell
   psf-guard-cli worker-service install
   ```

   To use a different settings file, add `--config FILE`.

The command installs and starts the **PSF Guard Worker** service. It copies
the executable to `%ProgramFiles%\PSF Guard Worker` and settings to
`%ProgramData%\PSF Guard Worker\worker.json`. SYSTEM, Administrators and
the service account can access the service's data folder. The settings
include the worker token.

The service runs under its own account, `NT SERVICE\PSFGuardWorker`.
Task files use `%ProgramData%\PSF Guard Worker\tasks`; the desktop app's
**Temporary task folder** setting does not change the service's task folder.

### Apply settings or app updates to the service

After changing worker settings, click **Save worker settings**. After
saving settings or updating PSF Guard, open PowerShell as administrator and
run `psf-guard-cli worker-service install` again. The command stops the
service, updates its copies of the app and settings, and restarts it.

**Restart worker** controls the desktop worker. It does not restart or
update the Windows service. While the service is running or changing state,
**Start worker** is disabled to prevent a second worker from starting.

### Check service logs and requirements

- **Logs:** filenames start with `worker` in
  `%ProgramData%\PSF Guard Worker\logs`.
- **Access denied:** if the service cannot read `worker.json`, update PSF Guard
  and run `psf-guard-cli worker-service install` from PowerShell opened as
  administrator. This repairs the service folder's permissions and starts
  the worker.
- **Failures:** if the server refuses the token, the service logs the reason
  and stops. Windows restarts it after 30 seconds. Replace a refused token
  in the desktop settings and run the install command again.
- **Stopping:** stop the service in Windows Services. The worker disconnects
  and the server queues unfinished tasks again. Windows shutdown also gives
  the worker time to disconnect.
- **Sleep:** prevent the computer from sleeping while you need it to process
  tasks. A worker cannot process tasks while Windows is asleep.
- **RC-Astro tools:** the service uses rc-astro from its account's `PATH`,
  or the executable specified in its settings. An installation or license
  available only to your desktop account may be unavailable to the service.
  If the service cannot run RC-Astro, it can still accept stack builds.
  Use the desktop worker if RC-Astro requires your desktop account.
- **Shared storage:** the default service account accesses network shares
  through the computer's network identity. If a share requires a user
  account, change the service account in Windows Services to a user with
  access to that share. Give that user modify access to
  `%ProgramData%\PSF Guard Worker`. Enter folder mappings with UNC paths,
  such as `\\nas\astro`, that the service account can access.
  Reinstalling restores the default service account. After reinstalling,
  stop the service, reapply your custom account, and start the service in
  Windows Services.

### Remove the Windows service

Before uninstalling PSF Guard, open PowerShell as administrator and run
`psf-guard-cli worker-service uninstall`. This stops and removes the service
and deletes its copied executable, settings including the token, task files,
and logs. The server queues unfinished tasks again. The desktop app's
saved worker settings remain in its own settings folder.

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
- A mosaic stacks on a worker too. The worker writes one calibrated
  reference frame per panel to its `--work-dir` while it plans the canvas,
  and deletes them when planning ends. Allow room for them: each is about
  the size of a 32-bit frame. A mosaic saves no checkpoint: **Stop**
  discards its progress, and the next build starts the mosaic over.
- Before a mosaic goes to a worker, the server checks that its frames
  cover every panel. Under **Workers first**, a mosaic that a worker
  reports as incomplete is not stacked again on the server, because the
  same frames would fail there too.

## Choose how files reach the worker

Task files transfer through the server by default (`--files remote`). The
worker downloads each input over the server's API and uploads each result.
It needs only the server's address. A 6000 × 4000 color image is about
290 MB each way.

Shared storage is optional. If the worker and server can access the same
storage, such as an NFS or SMB share, the worker can read inputs and write
results in that shared folder:

1. On the server, in **Settings › Workers**, open **Shared storage**. Enter
   a folder on the share as the server sees it, such as `/srv/astro`, and
   click **Save**.
2. Start the worker with a map from the server's path to the worker's:

   ```bash
   psf-guard worker --server http://nas:3000 \
     --map /srv/astro=/Volumes/astro
   ```

   Repeat `--map` for more folders. A map implies `--files shared`. With
   `--files shared` and no map, the worker uses the server's paths as they
   are. In the desktop app, open **Task files**, select **Use shared storage**,
   and enter each path under **Server folder** and **Folder on this computer**.
   Use **Add folder mapping** for more folders. Leave **Shared folder** on
   the server blank to use the default folder shown in Settings.

When the worker joins, the server writes a test file in the shared folder.
The worker reads the test file and writes a reply in the same folder. If
both checks succeed, the worker logs `Shared storage` and **Settings ›
Workers** shows **Shared storage**. If either check fails, the worker logs
the reason and downloads inputs from the server instead. A file the worker cannot see, or whose size does not match, also
transfers through the server.

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

- **Stop** on a processing run removes its task from the server's queue
  or cancels it if it is running. A tool on the server stops immediately. A worker stops its tool at its next progress
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
server disconnects the worker immediately and queues its tasks again. At its next
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
