> Implementation status: the paired narrowband worktrees implement shared
> H-alpha stretching, display balance, automatic SHO/HOO selection, and RGB
> star replacement on regular narrowband cards. Continuum subtraction remains
> a design for a later linear stage. The standalone HTML preview includes
> exploratory controls and is not the application.

# Color previews and channel processing

Status: **Built**: color previews follow each channel card's deconvolution
and RC-Astro steps by default, run them on each channel's own stack in the
card's cache, and can run BlurXTerminator and NoiseXTerminator on the color
channels together.
**Built in the paired feature worktrees**: narrowband balance, automatic
palettes, and RGB stars. **Proposed**: continuum subtraction for broadband plus
narrowband combinations, green reduction, and additional lightness choices.
Last updated: 2026-10-24

A channel card can run deconvolution and the RC-Astro tools
(BlurXTerminator, NoiseXTerminator, StarXTerminator) on its stack. A color
preview built from those stacks has had its own deconvolution and RC-Astro
settings for each input, off by default, and a person copied the cards'
settings across with a button. The tools then ran again for the color
preview, on each input after background extraction and registration, so a
BlurXTerminator pass tuned on a card ran twice and the color preview ran
nothing unless the person remembered to copy.

This record covers three steps:

1. Color previews take each input's tools from its channel card by default.
2. A channel's tools run once, on the channel's own stack, and serve both
   its card and every color preview built from it.
3. BlurXTerminator and NoiseXTerminator can run on the color channels together.

## The color pipeline

For each input, a color build runs, in order:

1. registration onto the reference channel (R for RGB, L for LRGB, H-alpha
   for narrowband), measured on the stacks as they came;
2. deconvolution and RC-Astro tools on the channel's own stack, in its
   channel card's cache (step 2);
3. background extraction on the channel in its own frame, with one surface
   model shared by all channels;
4. resampling onto the reference with the transform from step 1;
5. normalization;
6. RC-Astro on the color channels together, when set (step 3);
7. color calibration and the input's stretch stages;
8. composition, then the RGB output stages.

Before step 2, a build ran background extraction first, then registered and
resampled, then ran the tools on the registered channel. Those results were
filed under the color input (`color-input:{input}:{role}`) and the card's
under the stack (`mono:{job}:{group}`), so nothing was shared.

## Step 1: follow the channel cards (built)

`StackColorProcessing.input_processing` says where the inputs' deconvolution
and RC-Astro steps come from:

- `own`: the build's own `input_deconvolutions` and `input_rc_astro`, as
  before. A record without the field reads as `own` and is written without
  it, so color previews, saved defaults and named setups from earlier
  releases keep their results and their cache ids.
- `channels`: what each input's channel card applied to the exact stack
  revision the build uses. New builds and **Reset defaults** use this, and so
  do the built-in setups.

The server resolves `channels` when it prepares a build, after choosing the
source stacks: for each source it reads the card's applied processing the
same way the card shows it (the selected result, or the previous revision's
while its processing is carried to a refreshed stack) and writes its
deconvolution and RC-Astro steps into `input_deconvolutions` and
`input_rc_astro`, replacing any the request carried. From there the build is
an ordinary build with explicit settings: the same cache ids, the same job
manifest fields, the same progress phases. Automatic rebuilds keep the
choice, since they copy the previous build's processing, and read the cards
again.

The job records what it read in `followed_channel_processing`, one entry per
input, including inputs whose card applied nothing. The color catalog reads
the cards again and marks the build out of date when they differ ("a channel
card's deconvolution or RC-Astro steps changed"). It does not rebuild by
itself: an RC-Astro chain takes minutes, and a person may be tuning a card.

The job id hashes the processing with `input_processing` in it. A finished job
is reused as it stands and records whether it follows the cards, so a build
that follows the cards and one with the same tools as its own settings are two
jobs. The color inputs and RC-Astro chains key on the tools alone, so
switching between them only composes again. An `own` record leaves the field
out, so a build from an earlier release keeps its id.

In the **Processing stack** editor, **Deconvolution and RC-Astro** chooses
**From each channel card** or **Set in this build**. Following the cards,
each input lane names what its card applied in place of the tool editors.
Choosing **Set in this build** starts the build's own settings from what the
cards show, so nothing is lost. Named color setups store the choice.

## Step 2: share each channel's tools (built)

Each input's tools run on the channel's own stack, before background
extraction and alignment, with the ids the channel card uses:

- deconvolution: `deconvolution_cache_id(db, mono_source_key(job, group),
  revision, config)`, stored as the card stores it (`deconvolution/<id>/`);
- RC-Astro: `rc_astro_chain_ids(db, mono_source_key(job, group), revision,
  config, schemas, deconvolution_id)`, one cached result per chain prefix.

Whichever runs first, card or color build, stores the result and the other
reads it. This holds for `own` and `channels` alike: both end as
`input_deconvolutions` and `input_rc_astro`, so equal settings give equal
ids. One BlurXTerminator or NoiseXTerminator run serves the card and every
color preview that uses that stack. Background extraction now fits
sharpened, denoised channels, and alignment resamples sharpened stars.

Registration is measured on the unprocessed stacks, before the tools, and the
transform is applied to the processed ones. A tool setting therefore never
moves a channel. Every build now registers before background extraction,
with or without tools. A build without tools whose prepared inputs were
cached before this change keeps the transform it measured after background
extraction; it can differ by a fraction of a pixel from what a fresh build
measures. A test with a 3000 ADU gradient across one channel checks that
registration before extraction still finds the shift within the RMS limit.

Card runs and color builds that run tools both use the stack line, which has
one worker, so the same tool never runs twice at once. A color build whose
channel results are all cached already runs in the quick lane: it reads
files and runs no tool.

Identity and caches:

- `color_input_cache_id` hashes each processed channel's id (the last
  RC-Astro step, or the deconvolution) in place of the tool settings. With no
  tools it hashes nothing new, so color previews without tools keep their ids
  and caches.
- With tools, the job id and the input id also hash
  `COLOR_CHANNEL_TOOLS_VERSION`, and the job records it in
  `channel_tools_version`. A finished job with tools and an older version is
  out of date ("deconvolution and RC-Astro now run on each channel's own
  stack"); the next build runs them in the new order once. Version 2 came
  when rc-astro stopped reversing the rows of stacks without
  `ROWORDER = 'TOP-DOWN'`, such as WBPP masters: a version 1 build with
  RC-Astro is out of date ("RC-Astro could return a channel upside down"),
  and one with deconvolution alone is not.
  `STACK_COLOR_CACHE_VERSION` is unchanged: the latest-color index filters
  on it, and bumping it would hide every earlier color preview.
- The job lists its channels' deconvolution ids in `input_deconvolution_ids`.
  The cache janitor keeps those, as it already kept `input_rc_astro_ids`.
- The journal stores the color request, and a restart prepares the job again,
  so a resumed build gets the new ids. RC-Astro steps a stopped build
  finished stay cached, and so does a finished deconvolution.

The progress ledger follows the new order: loading, registration,
deconvolution, RC-Astro, background, then **Aligning channels to the
reference** (`aligning_sources`, one unit per non-reference channel).

### The result in PSF Guard

Teddy Bear on starfront-redcat61 again: the H-alpha, OIII and SII WBPP
masters as R, G and B, OIII turned 1.7° and shifted (23.4, −11.7) pixels,
and Seiza deconvolution (`conservative(5.0)`) on OIII. PSF Guard's own color
build ran the new order; the old order (register, resample, deconvolve) ran
on the same stacks in the same test. Both were normalized as a color input
and measured on the central 5448×3376 pixels. Registration found the same
transform in both.

| Deconvolution | Order | Stars | HFR | FWHM | Background σ |
|---|---|---|---|---|---|
| none | | 3054 | 2.740 | 6.449 | 0.00222 |
| conservative | before | 3160 | 2.696 | 6.346 | 0.00223 |
| conservative | now | 3172 | 2.696 | 6.346 | 0.00225 |
| strong (30 iterations, amount 1, max correction 4) | before | 872 | 1.981 | 4.663 | 0.00244 |
| strong | now | 1299 | 2.024 | 4.765 | 0.00232 |

With the default settings the orders match. With strong settings the old
order is 0.04 HFR sharper, but it detects 427 fewer stars and leaves more
background noise, as in the comparison below. A second build with other
settings read OIII's deconvolution back ("Reusing G deconvolution").

### The comparison that chose the order

Teddy Bear on starfront-redcat61: the WBPP OIII master registered to the
H-alpha master after a synthetic 1.7° turn and a (23.4, −11.7) pixel shift,
measured on the central 2400×2400 pixels. RC-Astro ran through its CLI on
supergateway with default BlurXTerminator and NoiseXTerminator settings.
A registers and then runs the tools, as color previews did before step 2; B
runs the tools and then registers.

| Tools | Order | Stars | HFR | FWHM | Ring dip | Background σ |
|---|---|---|---|---|---|---|
| none | | 234 | 2.695 | | | |
| BlurXTerminator | A | 139 | 2.630 | 6.191 | 2.62σ | 1.02 |
| BlurXTerminator | B | 184 | 2.595 | 6.108 | 2.07σ | 1.01 |
| BlurX + NoiseX | A | 135 | 2.631 | | 1.32σ | 0.78 |
| BlurX + NoiseX | B | 182 | 2.557 | | 0.81σ | 0.80 |

B gave tighter stars, weaker rings around them and more detected stars, with
the same background noise. Seiza's own deconvolution on the full frames
agreed: with light settings the two orders matched; with strong settings A
was sharper by 0.06 HFR, but its halos were about twice as bright and it
detected 205 fewer stars.

This is one target, one channel and a small rotation. A larger turn, or a
channel with more resampling, could favour registering first.

## Step 3: RC-Astro on the color channels together (built)

`StackColorProcessing.color_rc_astro` runs BlurXTerminator and
NoiseXTerminator on the aligned, normalized linear channels as one
three-channel image, after normalization and before color calibration, the
input stretches and composition. A build can run a tool per channel, on the
channels together, or both. A record without the field is written without
it, so earlier builds keep their ids.

**Why before composition.** The plan was the composed RGB image before the
output stretch, but the composition is linear only when calibration runs and
the channels share their stretch. A narrowband palette composes channels each
stretched on its own, and LRGB mixes in luminance. The aligned channels are
linear for every kind, and fringes and color noise are differences between
them, which the tools see there. RGB and LRGB send R, G and B (luminance has
no color); narrowband sends H-alpha, OIII and SII, and a palette without SII
sends OIII twice and keeps the first copy, since the tools take one channel
or three. StarXTerminator is refused: a starless color image needs its own
stars path through calibration and the stretches.

**Identity.** `color_rc_astro_chain_ids` keys the chain on
`color-channels:{linear_input_id}` and the roles sent, with the usual per-step
ids. The prepared inputs' id covers everything before the tools, so changing
calibration, a stretch, the crop or the palette's mix reuses the result. The
job id hashes the chain's last id, which carries the CLI and model versions;
a build without the field hashes nothing new. The job records
`color_rc_astro_id` and `resolved_color_rc_astro`; the janitor keeps the
result while the build is kept, and a missing result makes the build out of
date. A build whose chain is not cached runs in the stack line, not the
quick lane. Progress shows **Processing the color channels with RC-Astro**
after normalization, one unit per step.

**Correct only.** rc-astro's schema lists BlurXTerminator's `--correct-only`
switch without a type, so Seiza drops it and PSF Guard cannot send it. The
switch sets `ss`, `sn`, `ash` and `nsd` to 0, `ansp` on and `lp` off; sending
those values gave identical pixels to `--correct-only` on a 1200×1200 RGB
crop. The UI offers **Correct only** wherever the schema has all six, and
BlurXTerminator on the color channels starts with it on when it is offered.

### The comparison

Teddy Bear on starfront-redcat61: the R, G and B WBPP masters (already
registered by WBPP), a 2000×2000 crop near the top-left corner where the
refractor's off-axis stars are worst, each channel normalized as PSF Guard
normalizes a color input. rc-astro 2.6.6 ran on supergateway with default
settings except Correct only. 1605 isolated stars on G; centroid offsets
between channels in pixels, FWHM from second moments, spread = (largest −
smallest channel FWHM) / mean, color noise = standard deviation of R−G and
B−G over star-free 32×32 tiles.

| Run | R–B | R–G | G–B | FWHM R | FWHM G | FWHM B | Spread | Noise R−G | Noise B−G |
|---|---|---|---|---|---|---|---|---|---|
| none | 0.177 | 0.133 | 0.172 | 3.46 | 3.32 | 3.89 | 0.159 | 0.00119 | 0.00121 |
| BXT correct only, together | 0.152 | 0.097 | 0.140 | 3.19 | 3.22 | 3.67 | 0.123 | 0.00119 | 0.00121 |
| BXT correct only, per channel | 0.185 | 0.137 | 0.141 | 3.20 | 3.15 | 3.68 | 0.150 | 0.00119 | 0.00121 |
| BXT default, together | 0.144 | 0.087 | 0.128 | 2.95 | 3.01 | 3.52 | 0.144 | 0.00119 | 0.00121 |
| BXT default, per channel | 0.158 | 0.108 | 0.134 | 2.90 | 2.98 | 3.49 | 0.154 | 0.00120 | 0.00121 |
| NXT, together | 0.099 | 0.095 | 0.101 | 3.19 | 2.86 | 3.13 | 0.120 | 0.00056 | 0.00014 |
| NXT, per channel | 0.144 | 0.122 | 0.139 | 3.23 | 2.74 | 3.00 | 0.173 | 0.00058 | 0.00021 |

Run on the channels together, Correct only brought the channels' star
centroids closer (R–G 0.133 to 0.097 pixels) and their sizes closer
(spread 0.159 to 0.123); run per channel it moved neither. NoiseXTerminator
together removed more B−G noise than per channel. This is one target and one
optical system.

## Narrowband balance and RGB stars

Give a new narrowband preview a useful initial color balance without asking
the user to stretch H-alpha, OIII and SII independently. Keep **Narrowband
balance** separate from photometric color calibration and from continuum
subtraction. It changes the displayed contribution of emission channels;
it does not measure their physical abundance or remove continuum.

### Research and limits

Research checked on 2026-10-24:

- [Cosmic Photons' process descriptions](https://cosmicphotons.com/scripts/)
  distinguish NarrowbandNormalization's channel balance from NBColourMapper's
  layer mixing, which can also support continuum subtraction.
- The [NarrowbandNormalization documentation package](https://www.cosmicphotons.com/pi-modules/narrowbandnormalization/nbnDocumentation20240816v010001.tar.gz),
  listed in the author's [update manifest](https://www.cosmicphotons.com/pi-modules/narrowbandnormalization/updates.xri),
  describes H-alpha as the reference and background-protected midtone curves.
  It permits linear and nonlinear data, with Lightness restricted to nonlinear
  data. The document is version 1.0, dated 2024-08-16; the manifest also lists
  module 1.1. Do not claim parity with the installed process from these notes.
- [NightPhotons' PhotometricContinuumSubtraction documentation](https://www.nightphotons.com/software/photometric-continuum-subtraction/)
  describes fitting stellar fluxes on linear images, using one or more
  broadband references, and applying the fitted weights to starless images
  separately. Its starting limits are 400 stars and peak 0.8. The
  [script source](https://github.com/charleshagen/pixinsight/blob/main/src/scripts/NightPhotons/PhotometricContinuumSubtraction.js)
  inspected reports version 1.4.2. These are reference workflows, not new
  dependencies or code to copy into PSF Guard.

The mathematical operations belong in Seiza. Its
`docs/design/color-composition.md` records the numeric contracts and continuum proposal,
failure cases, bindings and validation. PSF Guard owns source selection,
setups, jobs, caches, diagnostics and this UI.

### Where the new steps run

The existing processing order remains the compatibility path for existing
previews. A new **Balanced narrowband** setup uses this order:

1. Load the chosen linear stacks and measure registration on their original
   pixels, as today.
2. Follow each channel card's deconvolution and RC-Astro steps. Reuse the
   card's native-grid results, then correct backgrounds and align channels.
3. Select the common valid area for statistics. Prepare the physical channels
   for display, recording every affine scale and offset. This is display
   preparation, not the new narrowband balance.
4. Run optional BXT/NXT on the aligned linear color channels, as today.
5. Apply one shared stretch to the physical H-alpha/OIII/SII planes, resolving
   its curve once from H-alpha. Independent auto stretches are an advanced
   alternative, visibly marked **Separate channel stretches**.
6. Resolve and apply **Narrowband balance** to OIII and SII with H-alpha as
   reference. Keep the physical role names even when a palette changes.
7. Map the balanced channels through a static palette, then run optional
   green reduction and output stretch stages. Preserve HOO's identical OIII
   copies when no synthetic-green mix is requested.
8. Crop and publish the preview with its resolved settings and warnings.

The first version supports static palettes, including HOO and all six
three-channel assignments. Foraxx already has its own dynamic display
mapping. Keep the new balance off for Foraxx until a real-data comparison
establishes a useful combination; explain this when the palette changes.
Never interpret Foraxx's mixed RGB channels as three physical filters.

This setup deliberately places balance in the display branch. The core can
support linear data, but a nonlinear balance must never feed continuum fitting,
photometry, stack accumulation or a download described as linear.

### Starting settings

The balanced narrowband column describes the initial recipe. Combined-image
settings remain proposed; broader real-frame tuning is still needed:

| Setting | Balanced narrowband | RGB/LRGB | LRGB + emission |
| --- | --- | --- | --- |
| Channel tools | From each channel card | Existing behavior | From each channel card, subject to linear-input checks |
| Background correction | Existing automatic protected model | Existing behavior | Same correction on the fit and subtraction inputs |
| Color calibration | Off | Existing Gaia calibration | Gaia on the broadband RGB branch only |
| Stretch | Shared H-alpha Auto-MTF, median 0.2, shadows −2.8 | Existing behavior | Shared stretch after linear emission blending |
| Narrowband balance | On, reference H-alpha | Off | Off for natural color; optional on the mapped SHO branch |
| OIII / SII boost | 1.0 / 1.0 | Not applicable | 1.0 / 1.0 on a mapped branch |
| Shadow protection | 1.0, the median background point | Not applicable | Same on a mapped branch |
| Green reduction | 0, optional after mapping | Off | Off for natural color |
| Lightness replacement | Off; defer additional choices | Not applicable | Existing LRGB luminance step |
| Continuum subtraction | Off without broadband references | Off | Automatic, with a visible outcome for each line |

Do not automatically remove stars or require an RC-Astro licence. Report when
stars are included in balance statistics. A later option can measure diffuse
signal through a star mask or use existing starless results; those modes need
their own validation and cache identity.

### Setups and compatibility

Add an explicit, optional `narrowband_balance` configuration and a separate
shared-input-stretch configuration to `StackColorProcessing`. Missing fields
mean the exact current behavior. Disabled configurations serialize as absent,
so an old request keeps its job id. Do not reinterpret `input_stretches` as a
shared resolved curve: identical Auto-MTF settings currently resolve separately
on narrowband inputs, which is a different operation.

Offer **Balanced narrowband** as a built-in setup. On a fresh installation,
the default Color setup may select it for static narrowband cards through an
explicit composition profile. An existing default Color setup, edited or
unedited, keeps its behavior. Users can opt in on a card and save that setup.
Do not rewrite the all-role `default_color.json` or decide a setup's meaning
from its display name. The current single default setup needs an explicit
per-composition profile before different automatic RGB and narrowband defaults
can ship. **Reset defaults** uses the user's selected default profile.
Use a recorded configuration migration marker to distinguish a fresh install.
If that distinction is unavailable, keep the current default and offer the
preset explicitly; an absent or unedited saved setup does not prove freshness.

Teach `colorSetupForRoles` and server setup validation about the new fields
together: the current mapper enumerates fields and would otherwise drop them.
Preserve an inapplicable profile in a saved setup, but do not execute it on an
RGB card. Explain **Narrowband balance applies to narrowband palettes**.

### Processing stack UI

The following placement supersedes the first editor-only mockup. Keep the
existing RGB, LRGB and narrowband cards under **Combine channel stacks**.
There is no additional **Combination** or **Result** selector. Put these
three controls in the narrowband card's header:

| Header control | Choices | New automatic setup |
| --- | --- | --- |
| Palette | Automatic, existing static palettes, existing Foraxx palettes | Automatic: SHO with usable SII, otherwise HOO |
| Stars | Automatic, RGB, Narrowband, None | Automatic: RGB when usable, otherwise Narrowband with the reason shown |
| Edges | Existing crop choices | Existing default |

Show the resolved result below them, for example **Automatic → SHO nebula +
RGB stars**. Explicit choices take precedence: SHO without SII needs attention;
it does not silently become HOO. RGB without usable RGB inputs or star
separation needs attention; it does not silently use narrowband stars.

**Processing stack** remains collapsed until opened. Its existing setup picker
and **Save as…** stay at the start of the editor. Tool choices, background
correction, stretches, balance and RGB output stages stay inside this editor.
Put continuum mode, fitted scales, shared/separate stretch selection, boosts,
shadow protection, green reduction and star-separation settings under focused
details sections. They are not extra header dropdowns. Selecting a setup changes
processing settings; Palette, Stars, Edges and selected sources remain choices
of this color card. Show an existing result's actual settings, not new defaults.

**Custom combination** stays collapsed below the editor. It lists **Nebula
inputs** (H-alpha/OIII/SII) and **RGB star inputs** (R/G/B). Unique sources are
shown as text. Show a source picker only when the role needs a choice. Use
the same stack families and exposure groups as current selectors; a short RGB
exposure need not match a long narrowband exposure. L does not supply RGB stars.
Source selection, concrete palette, star policy and processing stay associated
with the card across reloads and refreshes. The new **LRGB + emission** result
gets its own card when supported; it is not an SHO palette choice.

Keep **Processing stack**, **Deconvolution and RC-Astro**, the channel lanes,
named setups, **Save as…**, and **Apply processing stack**. Add controls in
their processing order rather than opening another editor:

- **Channel stretch**: **Shared, reference Hα** or **Separate channel
  stretches**. Shared mode edits one stretch stack. Keep the separate draft
  when switching back, and never silently replace custom channel settings.
- **Narrowband balance**: an on/off switch, OIII and SII boosts, and
  **Shadow protection**. Put the reference and statistics in a collapsed
  **Measured balance** section. Hide SII controls when the chosen palette
  uses only H-alpha and OIII. A boost of 1 means no extra boost after balance.
- **Green reduction**: an advanced output control, initially 0. Keep it
  separate from OIII so the user knows it acts on displayed green.
- The preview's status shows **Balanced to Hα**, **Off**, or **Skipped** with
  the affected channel and reason. For a weak OIII measurement, write
  **OIII balance skipped: too little signal above the background**. A
  missing measurement does not establish that OIII emission is absent.

Editing settings changes a draft. Applying it starts a tracked job and keeps
the current image until the new one is ready. A later downsampled preview can
give faster feedback using the same resolved full-frame plan; slider changes
must not run full-frame work in a request handler or queue one job per event.
Maintain image zoom and page scroll when details open or results arrive.

The accompanying [processing UI study](narrowband-processing-ui.html) is an interactive design
preview of the card header, collapsed editor and source choices in context.
Its scenario selector sits outside the product card and is only a testing aid.
Its outcomes and Build action are simulated; it does not process images.

### Automatic narrowband with RGB stars

This is a required supported result of the new automatic setup, not a later
manual-only option. Palette and star source resolve independently. Automatic
selects SHO when H-alpha, OIII and SII have usable sources, otherwise HOO when
H-alpha and OIII do. It never invents SII or interprets broadband R as H-alpha.

Automatic stars selects a complete, unambiguous R/G/B set when a configured
star-separation tool can produce both a starless narrowband branch and RGB
stars on the same grid. Respect explicitly selected sources. Reuse compatible
tool artifacts, including a saved star-containing chain prefix, but do not
assume that an existing starless output also supplies an RGB stars layer.
When only narrowband exists, keep its stars and explain **RGB stars unavailable:
no complete RGB source set**. With ambiguous RGB sources, offer the relevant
picker in **Custom combination** and report the fallback. Missing tools or a
failed separation also produce a visible fallback to narrowband stars in
Automatic. Explicit RGB or None instead fails the new build and keeps the
previous preview if separation cannot run.

Build the automatic result in this order:

1. Resolve the physical narrowband sources and any usable RGB star sources.
   Register all required sources onto the same reference grid and retain
   star-containing linear samples after channel tools/background correction.
2. Measure optional continuum weights while the inputs still contain stars.
   A failed optional fit in this automatic narrowband recipe leaves the
   original narrowband signal in use and reports that continuum remains.
   This differs from an explicitly requested emission-residual composite,
   whose required line failures still need attention.
3. Calibrate broadband RGB before extracting its stars. Produce a starless
   narrowband branch and an RGB stars branch with the configured tool. Keep
   their source revisions, masks, transfer and extraction settings. If the
   continuum fit applies to starless inputs, validate and record their
   association with the measured star-containing revisions.
4. Stretch the starless narrowband channels with the shared H-alpha curve,
   balance them, and map the resolved static palette. Foraxx uses its existing
   mapping on this branch with the new balance off.
5. Prepare the RGB stars with a consistent color-preserving display transform,
   recombine them onto the mapped starless nebula, and run RGB output stages
   on the complete image. This replaces narrowband stars; it does not add a
   second set over a star-containing nebula. Validate mask edges and halos.

Star source and continuum subtraction remain independent: choosing RGB stars
does not blend broadband nebular structure or require a successful continuum
fit. The automatic recipe attempts continuum fitting when usable references
and valid linear measurements exist and the requested recipe can produce a
starless nebula branch. If Automatic falls back to retaining narrowband stars,
keep the original narrowband signal and report that continuum was retained
too. Subtracting continuum from a star-containing image would otherwise change
the very stars this fallback promises to keep. A later recipe that subtracts
continuum while preserving narrowband stars must extract, preserve and restore
that stars layer explicitly.

Persist `palette_selection` and `star_selection` policies separately from the
resolved palette and source roles. Proposed names are subject to API review.
Resolve Automatic during job preparation, record why it chose each branch,
and include the resolved plan and star-tool/model versions in cache identity.
Record the relevant source-availability fingerprint as well: an added SII or
RGB source can change an automatic result even when its previous source
revisions did not change. Availability checks only mark the result out of date;
they do not run star separation or image processing in a catalog request.
Source updates can change an automatic HOO result to SHO or enable RGB stars;
explicit choices do not change. Rebuild once the source changes settle, as
current refresh does, and show what changed. Missing policy fields preserve
the old concrete palette and narrowband stars. Do not add Automatic to Seiza's
physical `NarrowbandPalette` enum: source selection is PSF Guard's job.

## Proposed: subtract continuum for combined images

Support two explicit results rather than treating LRGB and SHO as interchangeable:

- **LRGB + emission** retains broadband colors and adds continuum-subtracted
  H-alpha, OIII and SII residuals. This is the proposed initial combined-image
  default. Start with an emission amount of 0.25 as an artistic preset, with
  each line independently adjustable; validate that starting amount on real
  data before shipping it.
- **SHO + broadband** uses mapped line emission for the nebula, a separate
  broadband-structure contribution, and optional RGB stars. Start that preset
  with SHO structure alone and make the broadband amount explicit. Use the
  same automatic/explicit RGB-stars path as the narrowband card. Never label
  a star-containing RGB blend “RGB stars.”

Add explicit composition kinds and typed blend settings for these results.
Do not infer the result from the presence of seven input roles. Initially the
user chooses a combined mode; the existing automatic first-color selection
continues to choose LRGB, RGB or narrowband as documented. A combined result
the user builds can then follow its sources through the existing refresh flow.

Luminance L contributes structure. It is not the default continuum reference:
its broad response mixes several lines. Start with R for H-alpha and SII,
and a measured G/B composite for OIII. These are candidate sets, not fixed
spectral coefficients. Fit G/B weights from the stars; do not hard-code a
50/50 OIII continuum. Ambiguous sources or unknown filter roles require a
choice in **Custom combination**. Broadband and narrowband exposures can
differ substantially, so resolve exposure groups within each family rather
than requiring all seven roles to share the existing factor-of-two band.

### Linear preparation and subtraction

1. Resolve exact source revisions for the broadband and narrowband branches.
   Align them to L when supplied, otherwise R. Use a common valid mask.
2. Keep the background-corrected, aligned linear samples before independent
   percentile scaling and before any stretch. The current normalized-input
   cache is too late to be the only input to this operation.
3. Detect and match isolated, unsaturated stars. Measure local-background
   aperture fluxes on the exact pixel revisions used for subtraction. Fit a
   positive continuum scale, or a constrained nonnegative multi-reference
   model. Reject outliers and check spatial and brightness-dependent residuals.
4. Check PSF compatibility as well as registration. For an initial release,
   skip automatic subtraction when stellar residuals fail validation rather
   than sharpening or blurring the user's stacks without a recorded choice.
   PSF-matched subtraction is a later explicit preparation step.
5. Subtract the fitted continuum in linear light. Keep signed floating-point
   residuals and coverage masks. Do not clip negative noise values, recenter
   the residual by its median, or normalize each line to equal brightness.
6. Convert residuals into the continuum reference's working units before
   applying artistic amounts. For a single reference with fitted scale `a`,
   that is `E/a`; the multi-reference plan records its composite reference
   scale explicitly. Calibrate broadband RGB, transform residual units
   consistently with the calibrated channel gains, and blend the line
   contributions. Add L through
   the existing luminance step. A nonnegative display contribution is an
   explicit rendering choice; keep the signed residual separately.
7. Stretch the combined natural-color result together. For mapped SHO, stretch
   and balance the emission branch before its palette mapping, and then blend
   the separately prepared broadband structure and optional stars.

First-pass photometry requires star-containing linear channels. If a followed
card applies StarXTerminator, use a validated pre-removal chain prefix only
when the subsequent starless branch is explicitly supported; otherwise report
**Continuum subtraction skipped: star-containing linear inputs are required**.
Never fit a continuum coefficient from starless images. Reusing a coefficient
across a tool that changes flux requires evidence of equivalent photometry.

Broadband filters also transmit emission lines. A stellar continuum fit alone
does not correct that contamination. Without known transmission responses,
describe the output as an **emission residual**, not a pure line-flux image.
Do not infer throughput from filter names, exposure times, gain or bandwidth
alone. A later throughput-aware solve can correct line attenuation explicitly.

### Automatic outcomes and recovery

**Continuum subtraction** offers **Automatic**, **Manual**, and **Off**.
Automatic records an outcome for each requested line. Start with at most 400
candidate stars; minimum usable stars, fit-error limits and PSF thresholds are
provisional until synthetic and real-frame tests set them. Interpret any peak
limit against a known saturation scale; the existing arbitrary percentile
white point is not a sensor saturation level.

Show **Continuum fit** with its reference channels, fitted scales, stars used,
rejected stars, residual plot and any warnings. Provide a signed residual
preview and download, plus the original narrowband preview for comparison.
Manual edits the fitted scale or reference weights; a blank field means use
the measured value. Never report a manually overridden coefficient as fitted.

If one line fails in **LRGB + emission**, omit that line and show **Ready with
warnings**, listing what was added and skipped. The unmodified broadband
branch remains usable. Do not silently add the unsubtracted narrowband image.
If a required SHO line fails in **SHO + broadband**, keep the previous result
and mark the new build as needing attention; offer an explicit choice of a
different palette or **Off**. Changing **Automatic** to **Off** explains that
the original narrowband channels will be used, including their continuum.

### Jobs, cache identity and implementation sequence

Retain the shared job line, stop controls, priority, worker leasing and
`ComputePool`. Fit and image operations belong in the job runner. Add phases
for **Measuring continuum**, **Subtracting continuum**, and **Balancing
narrowband channels**; write all results atomically below the database slug.

Cache boundaries follow data dependence:

- Registration and native channel tools stay reusable across these choices.
- Add an aligned linear cache before display normalization. Its key includes
  sources, revisions, background correction, transforms, tools and the mask.
- A continuum plan keys on those inputs, candidate references, fit options,
  masks, any PSF preparation and the algorithm version. Persist the resolved
  coefficients and diagnostics. A display crop may reuse it; a change to the
  measurement region must resolve it again.
- A display-balance plan keys on the prepared physical planes, shared or
  separate stretch plans, statistics region, balance options and version.
  A static palette permutation can reuse physical-channel balance.
- The final result keys on all plans, blend settings, source family, crop,
  output stages and the published Seiza output version. Keep dependencies in
  the manifest so the janitor retains every referenced artifact.

Deliver in reviewable increments:

1. Add Seiza's independent balance operation, reports, C/Python/CLI surfaces,
   deterministic tests and output-version coverage. Compare real SHO/HOO
   masters, including stars, faint OIII, gradients and cropped overlaps.
2. Consume the published Seiza release in PSF Guard; add setup fields, the
   shared stretch, controls and reports. Check old JSON, job ids, default
   setup preservation, setup import/export and role mapping. Cover static
   palette changes and separate stretches explicitly. Include automatic RGB
   stars with the complete extraction/recombination path, policy resolution,
   missing-source/tool outcomes and explicit-choice failures in this increment.
3. Add Seiza continuum fitting and signed subtraction, then PSF Guard's
   **LRGB + emission** sources, cache and outcome UI. Test known coefficients,
   line contamination, mismatched PSFs, saturation, few stars and ill-conditioned
   G/B references before enabling Automatic by default for this new kind.
4. Add mapped SHO plus broadband structure, reusing the already supported
   automatic RGB-stars branch. Validate luminance and structure blending
   independently of star replacement.

The paired development branches are `codex/narrowband-normalization` in
`psf-guard-narrowband-normalization` and `seiza-narrowband-normalization`.
Local Cargo patches can exercise a paired change without committing machine
paths; PSF Guard's committed dependencies continue to use published crates.
Before handoff of behavior changes, run the focused Rust and UI regression
checks, lint/build, and the relevant browser tests through `e2e-local.mjs`,
including WebKit scroll coverage. Record the actual checks in each PR.


### Build the paired feature worktrees

The prototype currently depends on the new Seiza stacking API in version
0.47.0. Build PSF Guard against the paired Seiza checkout with the local Cargo
patch file generated for this worktree:

```sh
cargo build --config target/narrowband-local.toml
```

That file is a local development artifact under the ignored target directory.
The development lockfile resolves Seiza to local paths. Before merging the
PSF Guard dependency update, publish Seiza 0.47.0 and regenerate Cargo.lock
against the registry. Do not publish the development lockfile as a release
resolution. The implementation has no claim of numerical parity with the
Cosmic Photons PixInsight script.


### Real StarXTerminator validation

The RGB-star path was exercised on Supergateway with licensed RC-Astro 2.6.6,
StarXTerminator ML11, using its CPU backend. The isolated server used central
1024×1024 cutouts of existing Bubble Nebula R/G/B and Ha/OIII/SII masters;
Seiza read the sources and PSF Guard registered all six planes onto Ha.
The original masters and live catalog were not changed.

Explicit RGB, Automatic and None each completed through the normal tracked
color-job API. The first RGB build ran SXT on RGB and the physical narrowband
planes. Automatic resolved to RGB and reused both result IDs, with no further
SXT invocation; its cached composition took about two seconds. None ran one
narrowband separation and produced the starless preview. All three downloads
were display FITS and reported applied OIII/SII balance.

Full-coverage cropping retained 752×936 pixels (67.1%) with RGB stars, compared
with 969×954 (88.2%) for the starless narrowband-only branch. This checks that
RGB registration gaps affect the final crop. These images have different crop
origins and sizes, so a direct pixel-by-pixel comparison needs their recorded
crop regions. RGB star calibration reported no plate solve: the cutout fixture
has no fresh pixel solve. This run verifies separation, recombination, caching,
and coverage, rather than Gaia calibration or full-frame processing speed.
