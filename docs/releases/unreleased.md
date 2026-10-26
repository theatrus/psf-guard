# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Upgrading

- **Allow time for rebuilds.** The stacking engine changed, so each stack
  rebuilds once when next opened or refreshed.
- **Scoring defaults are now one copy on the server.** The first editor to
  open PSF Guard after the update saves their own scoring defaults there,
  built-in values included. Everyone on the server then sees the same
  defaults, and only an editor can change them. A viewer's own defaults are
  not copied.

## Changes

- Settings groups related pages in a desktop sidebar and a compact phone
  selector. Find GPU controls under **Performance**, exposure templates and
  processing setups under **Presets**, and export defaults, PixInsight and
  AstroBin under **Exports & integrations**. Existing settings and shortcuts
  are preserved. Settings asks before switching pages with unsaved edits.
  See [Settings navigation](../SETTINGS.md).

- Mosaic color calibration now solves the integrated canvas instead of looking
  for a single reference-frame solve. RGB, LRGB, RGB-star and OSC calibration
  reuse that pixel solve. Older uncalibrated previews are marked out of date;
  **Build current set** refreshes them from the cached stacks.

- Mosaic SNR plots report canvas coverage and mean pixel depth. They keep
  each checkpoint's measured signal and no longer apply same-field forecasts
  or plateau/degrading verdicts to frames covering different sky areas.
  Saved mosaic plots use the corrected interpretation without rebuilding.
- A live stack image replaces the waiting placeholder, so mosaic previews
  no longer display a second, full-height spinner area beneath the image.

- FITS downloads in stack processing use compact controls with a download icon
  and explicit labels. Long labels wrap to fit narrow panels.

- Windows worker services can now read their saved settings and start
  successfully. Reinstalling the service also repairs permissions left by an
  earlier installation and waits for the stopped worker process before
  replacing its executable.

- A fresh flat that contributes to a light's master now provides coverage even
  when older flats in the same master move the median capture time beyond
  the **Flat age limit**. Age checks and the calibration report use the nearest
  contributing flat; a 1-day limit includes exactly 24 hours on either side.

- Color processing now follows numbered, expandable steps from input preparation
  to RGB finishing. See color and RGB-star branches separately, inspect the
  resolved star policy, and edit saved setups with consistent field fonts and
  spacing. Emission amounts and star reduction appear where they run in the
  pipeline; closing a step preserves its edits.

- Run a worker from the PSF Guard app on macOS or Windows. Open
  **Settings › Workers › Worker on this computer**, enter the **Server
  address** and **Worker token**, then click **Save worker settings** and
  **Start worker**. The worker continues running in the menu bar or system
  tray after you close the window. **Start worker at login** starts it when
  you log in. Opening PSF Guard while its window is open brings that window
  forward instead of starting a second copy.

- Run a worker as a Windows service to process tasks before anyone logs in.
  Open **Settings › Workers › Worker on this computer**, click **Save worker
  settings**, then run `psf-guard-cli worker-service install` in PowerShell
  opened as administrator. Run the command again after changing settings or
  updating PSF Guard to update and restart the service. Desktop worker
  controls do not control the service.

- Run a worker as a Linux service. The Fedora RPM and the Debian package
  install a `psf-guard-worker` systemd service, not enabled. Enter the
  server's address and a worker token in `/etc/psf-guard/worker.conf`, then
  run `sudo systemctl enable --now psf-guard-worker`. On Debian, copy
  `/usr/share/doc/psf-guard/worker.conf.example` to that file first.

- Stack builds on another computer. Start a worker with `--work stack`, then
  choose **Workers first** or **Workers only** for stack builds in
  **Settings › Workers**. Each channel stacks there from the frames, masters,
  checkpoint and settings the server would use. Mosaics stack there too.
  Each channel's card shows which worker stacked it, and its **Live stack**
  preview shows the worker's running image. Stack builds stay on the server
  unless you choose otherwise.

- Run RC-Astro tools on another computer. Add a worker in **Settings ›
  Workers**, then run `psf-guard worker --server …` there with its token.
  The worker runs BlurXTerminator, NoiseXTerminator and StarXTerminator for
  stack previews, color previews and narrowband stars, and the processing
  panel shows where each tool runs. A worker accepts every tool it has, or
  the ones `--work` lists, and runs up to `--slots` steps at once. Choose
  under **Where work runs** whether RC-Astro runs on workers first with this
  server as the fallback, on workers only, or on this server only. Files go
  through the server, or, with `--files shared`, through a folder the worker
  and the server share.
  See [Workers](../REMOTE_WORKERS.md).

- Build one integrated mosaic per channel in **Stacks**, then compose RGB,
  LRGB and narrowband color from those mosaics. Frames keep their calibration
  sessions and align against overlapping panels. Incomplete mosaics identify
  panels with no aligned frames. Local background normalization also matches
  panel backgrounds through Seiza to reduce seam lines.

- Watch a channel's in-progress stack in **Stacks**, with a live preview updated
  about every five seconds. The finished preview replaces it when processing
  completes; completed FITS downloads remain separate during a rebuild.

- Narrowband previews can reduce the restored RGB stars independently of BXT.
  Adjust **Processing stack → Combine and restore stars → RGB star reduction** to make stars smaller and
  fainter while preserving their color and the starless nebula. Zero preserves
  the current result. BXT sharpening and halo controls remain configurable.

- Combined color previews validate held-out stellar pixel profiles and can
  explicitly match continuum PSFs with Gaussian broadening. Reports show
  core/halo errors and the changed resolution. Narrowband controls also offer
  star exclusions for balance statistics and green-excess reduction before
  RGB stars are restored. Optional calibrated-response exports retain signed
  line flux, exact source provenance and uncertainty assumptions.
- RGB-star previews now honor the selected Gaia settings and separate their
  SXT caches by those settings. Natural RGB/LRGB emission builds explain when
  an inherited SXT step cannot run. Clearing the optional OIII B/G ratio
  returns to measured photometry.

- Narrowband previews can sharpen their RGB star branch with BlurXTerminator
  before star extraction. Open **Processing stack → Process linear color → RGB star processing**.
- Custom **RGB + emission**, **LRGB + emission**, and **Narrowband + broadband
  structure** cards fit and subtract continuum on aligned linear inputs.
  Automatic, Manual and Off controls retain signed residual downloads and
  show per-line fit results. Failed natural-color lines are omitted with a
  warning; a failed mapped line keeps the previous preview available.

- New narrowband color cards offer automatic SHO/HOO selection, a shared
  H-alpha stretch, and background-protected OIII/SII balance. Choose
  **Stars → Automatic** to use RGB stars when the sources and StarXTerminator
  are available, with an explained fallback to narrowband stars. Exact RGB
  sources can use different exposures under the card's **Custom combination**.
  Existing previews keep their saved palette and processing.

- Automatic stack refreshes now compose a target's first color preview once
  its channels are stacked, as a build you start does. New color previews
  start from the Color setup marked as the default. PSF Guard adds one named
  **Default** with the current pipeline. To use another pipeline, open
  **Settings → Presets → Processing setups** and click **Make default** on a Color setup, or save
  your changes over **Default**. Color previews you already have are not
  changed.

- Save storage limits, folders, and WBPP retention together with **Save storage settings**.
  WBPP shares the **Limits** and **Folders** sections with the other storage options.

- Manage WBPP work files under **Settings → Storage & backups → Folders & limits**: choose a
  dedicated folder and volume usage limit, remove intermediates before masters,
  and set separate retention periods. Intermediates default to seven days and
  masters to 90 days; **Keep WBPP masters forever** protects masters from cleanup.
  Active runs, downloads, and master copies are protected, and saved process
  masters and imported stacks remain separate.
- Volume usage limits now accept every whole percentage from 0% through 100%,
  including values below 50%, for the cache, stacks, calibration masters, and WBPP.

- Stack previews now run on the GPU when your computer has one: Metal on a
  Mac, Vulkan on Linux and Direct3D 12 on Windows. Debayering, star
  detection, resampling, the live stack and the final pass run there. On an
  Apple M4 Max, 35 frames of 26 megapixels stacked in 5–6 seconds instead of
  12. Computers without a supported GPU, and frames under about a million
  pixels, stack on the CPU as before. To choose which passes run on the GPU,
  open **Settings → Performance → GPU**. With **Use the GPU** off, all stacking
  work stays on the CPU, including artifact searches and the channel
  alignment of color previews.
- In the Settings sections **Exports & integrations → PixInsight**, **Stacking → Stack
  previews**, **Performance → CPU & memory → Processor use** and, in the desktop app,
  **Server**, fields no longer appear blank, or show an Automatic port, for a
  moment while Settings loads. When
  the server reports new values, fields you have not edited update to match,
  and a field you are editing keeps what you typed. In PixInsight, **Check
  again** now updates the executable and WBPP runs folder.
- Settings no longer shows a second scrollbar when you open it from the
  Stacks page. Before, with a stack's **Exact measurements** table open,
  scrolling in Settings could also scroll the Stacks page behind it, and the
  page could be out of view when Settings closed.
- A plan's scheduling limits on its **Rigs** tab, and a rig's planning site
  and Target Scheduler limits under **Rigs**, no longer appear empty or
  marked unsaved for a moment when they load. A collaboration connection's
  **Automation** tab shows the saved choices at once instead of unticked
  boxes. **Stack with WBPP** shows the project's process folder at once.
  Fields you have not edited now follow newer values from the server; your
  edits stay until you save or discard them.
- When someone else changes a rig's planning site or Target Scheduler limits
  while you edit them, **Save rig profile** no longer overwrites their
  change. PSF Guard saves the rest of the rig profile and explains that the
  planning site and Target Scheduler limits changed. Click **Reload** to load
  their values in place of your planning site and Target Scheduler limit
  edits. **Reload rig profile** keeps those edits when nothing changed on the
  server.
- Automatic stack refreshes now score with the server's scoring defaults,
  the same values every view starts from. Stacks that showed **Out of date —
  scoring settings changed** after an automatic refresh are rebuilt with the
  defaults on the next automatic refresh, and stop showing it. Saving new defaults in **Settings → Review → Grading & display**
  rebuilds stacks scored another way when automatic previews are on.

- New automatic narrowband recipes stretch the linear RGB stars independently
  with a SetiAstro-style Star Stretch. Lower the amount in **Combine and restore
  stars** to keep stars fainter. Saved setups keep their existing Auto MTF
  rendering until you switch.
- Narrowband cards show a visible ✨ Stars label beside the star-policy selector.

- Configure StarXTerminator separately for the linear nebula and RGB-star branches in **Process linear color**. Finished compositions retain the cached linear starless/stars outputs, and display blend amounts reuse the SXT runs.
- Set the processing memory budget in **Settings → Performance → CPU & memory → Processing
  memory**. Color and stretch errors show the estimate and current budget, with
  the setting to adjust it.

- Worker settings distinguish the worker on this computer from workers connected to the server. Labels explain task types, concurrent tasks, temporary files and saved tokens; tray status distinguishes waiting for a connection from waiting for tasks.
