# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- **Processing tools** has its own page, **Settings › Processing ›
  Processing tools**, instead of sitting at the bottom of **Stacking**.
  **Reference catalogs** links to it, since the noise model and ONNX
  Runtime are not catalogs.
- When **Processing steps** in **Settings › Performance › Workers** is set
  to **Workers only**, **Processing tools** lists a tool installed only on
  the server, such as the Seiza noise model, as **Not used**, explains why,
  and offers **Open Workers**. Before, it showed **Not installed** however
  many times you clicked **Update**, and a stack that used the model failed
  with a message that told you to download it again. The same applies to a
  tool only a worker has when the setting is **This server only**.
- **Rig setup** has a **Target Scheduler templates** tab. It lists the Target
  Scheduler exposure templates in the rig's database and lets you change
  their gain, offset, binning, readout mode, default exposure, twilight,
  humidity limit, dither interval and Moon avoidance. Before, you had to
  change them in N.I.N.A.
- You can delete a Target Scheduler exposure template that no exposure plan
  uses from **Rig setup › Target Scheduler templates**. The rig's next push
  does not bring it back, other PSF Guard catalogs delete their copy when
  they sync, and the N.I.N.A. Sync plugin deletes it in Target Scheduler at
  its next **Pull planning** once you update the plugin.
- The noise model shows as ready as soon as its download ends. Before, the
  page could keep showing it as not installed until you reopened it.
- A project's color stacks have **Open *target* in PSF Studio** for each
  target there that has channel stacks. The link opens PSF Studio, the separate
  editor for color processing, which fetches the target's stacks from this
  server. PSF Studio must be installed on the computer you browse from, and
  it is still experimental. The link carries no sign-in, so PSF Guard shows
  it only on a server that needs none. For a server that needs sign-in, run
  `psf-guard studio` with `--token` on the computer you browse from; see
  the stacking guide.
- PSF Studio, the experimental color editor, now runs an input's
  deconvolution before its processing tools, can subtract the continuum
  under Ha, OIII and SII from the R, G and B stacks (using each stack's
  clipping map from PSF Guard), and crops the picture to the area every
  input covers, or to a rectangle drawn with its new Crop tool. Mask
  editing gains a brush size, Erase, Undo stroke and Blur, and a run that
  prepares a picture can be stopped. A progress bar shows the tool that is
  running and how far it has got. Star removal can run on each input alone,
  or after registration on R, G and B together or on the narrowband lines
  together.
