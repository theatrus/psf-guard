# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- PSF Guard now uses Seiza 0.57. Seiza's stacking output changed, so stacks
  and color previews made before this update are stacked again the next
  time they are needed. When an expanded mosaic is reintegrated, its panel
  backgrounds now match. Processing tools such as RC-Astro now receive the
  few star-core pixels of a stack that lie above full scale at full scale,
  as PixInsight does, so their results are made again too. Update every
  remote worker to this release too: a
  worker running an older PSF Guard is not given stacking work until it is
  updated, though it still runs processing tools.
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
  editor for color processing, on the target's newest Studio document, or a
  new one from the target's stacks. PSF Studio must be installed on the
  computer you browse from, and it is still experimental.
- PSF Studio connects to PSF Guard servers. Click **PSF Guard** in its top
  bar and enter the server's address; on a server with sign-in, allow the
  code on the **Connect PSF Studio** page that opens in your browser. Studio
  then lists the server's projects, targets and Studio documents and opens
  them, and **Open *target* in PSF Studio** works on servers with sign-in
  too. **API tokens** in Settings lists each connection; **Revoke** signs it
  out. A connection can read, and save Studio documents when your account is
  an editor, and nothing else. It stays while PSF Studio uses it and ends
  after 90 days unused.
- PSF Studio saves its documents to the project a few seconds after you stop
  editing, with the file an external editor saved and, unless you turn off
  **Also save to the project**, its exports. Another computer opens the same
  picture; tool results are computed again there. When two computers save
  the same document, the second keeps its edits and offers **Save as a copy**
  or **Open the newer version**. Quitting PSF Studio saves to the server
  first.
- A project's stacks page lists its PSF Studio documents, with a preview,
  who saved each one and from which computer, the exports to download,
  **Open in PSF Studio**, and for an editor **Save as a Color setup** and
  **Delete**. PSF Studio sends the preview as it saves, so a document you
  never exported has one too.
- PSF Studio's document list shows how long ago each document was saved and
  from which computer, and warns when another computer saved it in the last
  ten minutes. Click a document to list its earlier revisions and open one.
- Name your PSF Studio documents: click the name in PSF Studio's top bar to
  rename one, or click **Rename** beside it on the project's stacks page. A
  document no longer takes a stack file's hash as its name: it starts with
  the target's name, or “Untitled picture”, and one named by a hash before
  shows as “Untitled picture” until you rename it.
- PSF Studio has a **Setup** row at the top of its layer list. Choose it to
  see the whole picture's setup in place of the canvas: the recipes that
  fit your inputs, each input's tools and the parts it goes to, and a card
  for each part with its blend, palette, order and **Remove part**. Every
  change there is an ordinary edit of the layers, which **Undo** reverses.
- PSF Studio can limit a layer, an adjustment or a step by brightness:
  **Mask by brightness** under Mask sets **From**, **To** and **Softness**.
  A layer that shows an input follows its own brightness; an adjustment or
  a step follows the brightness of the picture it changes. Steps on the
  picture with such a mask are drawn now instead of left out.
- When a target has no channel stacks yet, PSF Studio says so and offers
  **Build stacks in PSF Guard**, which opens the target's Stacks page,
  instead of failing as it prepares the picture.
- PSF Studio says when a document's stacks were built again since it was
  saved, and asks before opening one whose stacks were registered to
  another frame, where masks and heals may fall in the wrong place.
- `psf-guard studio` reads its token from `PSF_GUARD_TOKEN`, which other
  users of the computer cannot see, before `--token`.
- PSF Studio, the experimental color editor, now runs an input's
  deconvolution before its processing tools, can subtract the continuum
  under Ha, OIII and SII from the R, G and B stacks (using each stack's
  clipping map from PSF Guard), and crops the picture to the area every
  input covers, or to a rectangle drawn with its new Crop tool. Mask
  editing gains a brush size, Erase, Undo stroke and Blur, and a run that
  prepares a picture can be stopped. A progress bar shows the tool that is
  running and how far it has got. Star removal can run on each input alone,
  or after registration on R, G and B together or on the narrowband lines
  together. Continuum subtraction from starless lines no longer stops at
  Seiza's check of star profiles, which only matters when the stars stay.
  **Show › Only** on a layer shows that layer by itself: one
  input in gray, or a group such as R, G, B in color. Studio now draws output
  stretches of the combined picture, with **Linked**, **Each channel** or
  **Luminance** for the channels. Studio resolves a stretch that depends on
  the picture, such as the automatic midtones stretch, from samples of the
  picture at full resolution. PixInsight's histogram transformation is
  among the stretches. **Finish › Add step › Tone and color** has Camera Raw's
  Exposure, Contrast, Highlights, Shadows, Whites, Blacks, Vibrance and
  Saturation. The Final picture and each Finish step show the picture's
  histogram with the share clipped to black and to white. **Export…** on the
  Final picture saves the whole picture at full resolution as a 16-bit TIFF
  or PNG, or a 32-bit FITS or XISF, rendered on the CPU from the stacks'
  32-bit samples so it keeps more precision than the screen.
- In PSF Studio each processing tool, such as BlurXTerminator, is a step of
  its own. You can turn one off, remove it, or move it before or after
  Background extraction with **Move earlier** and **Move later**, for one
  input or for every input. Documents saved before open with their tools
  split into these steps, and Studio reuses the tool results it already
  has. **Undo** and **Redo**, in the toolbar and the Edit menu (⌘Z and
  ⇧⌘Z), go back through the document's edits. Every input stretch has a
  **Sky floor**, which takes what background extraction leaves to black,
  and **Run tools** during a run queues one more run with your later
  changes. A sharpening step's **Second pass, shifted** runs
  BlurXTerminator again on the image moved by half a tile and averages the
  two, which takes most of the faint grid of tile seams out of a dark sky.
  A tool can also run on one layer (**Run a tool on this**), on the image
  as that layer's steps before it leave it.
- In PSF Studio the steps on the whole picture are adjustment layers: an
  output stretch, Tone and color, Curves, a heal or a tool. **Add step**
  and **Add tool** put one on top of every layer, and you can drag it
  anywhere in the stack to change only the layers beneath it. Its
  visibility, opacity and mask work as for any layer. A star removal adds
  a layer above it, “Stars it took out”, screened over the picture, which
  you can hide, fade or move. Documents with Finish steps open with those
  steps as layers on top, in the same order, so the picture is unchanged.
- In PSF Studio, Option-click with the **Heal** tool sets where the heal
  copies from, and later strokes copy from the same distance. Select
  **Star removal** and heal to fix the starless picture: the heal becomes
  a layer right above it, beneath the stars. Select the **Stars it took
  out** layer and heal to remove or fix a star there alone.
- In PSF Studio a heal is matched to the brightness around it, and the area
  it copies from shows over the canvas. A group can be healed, and a Heal
  layer can have a mask of its own.
- PSF Studio's groups can hold groups, four deep, and a group's layers can
  pass through, each blending straight onto the layers beneath. A group can
  be clipped. A layer behind a shared mask can have a painted mask as well,
  and an adjustment of one layer can be behind a shared mask.
- In PSF Studio a tool layer inside a group runs on the group's layers
  beneath it, and an output stretch with layers above it is worked out from
  the layers beneath it.
- PSF Studio stretches RGB stars on the canvas, so the **Star stretch**
  amount redraws without preparing the store again. It also draws the
  narrowband balance and the green reduction live: turn the balance on and
  move its boosts in the Narrowband step, and add **Green reduction** like
  any other adjustment.
- BlurXTerminator's shifted second pass now measures how far apart its
  seams are and, when it moved by the wrong amount, runs once more moved by
  half of that, which removes more of the seam pattern.
- PSF Studio has a **History** panel at the bottom of the tree: each edit
  in a few words, such as "Ha: opacity 100% → 60%". Click an edit to go
  back to it; later edits stay until you make a new one.
- **Open in external editor** in PSF Studio's Final picture opens the
  picture as a 16-bit TIFF in its default app, such as Photoshop, and
  brings the edit back when you save it there. The Combined picture lists
  every input's tools in one table.
