# Color previews and channel processing

Color previews implement channel-card processing reuse, color BXT/NXT,
narrowband display balance, RGB stars, explicit combined recipes, continuum
validation and optional calibrated flux exports through Seiza 0.43.0 and
stacking/output 0.50.0.
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

Seiza owns image mathematics. PSF Guard owns source selection, saved setups,
tracked jobs, caches and the interface. See Seiza's
`docs/design/color-composition.md` for the binding and numeric contracts.

Ordinary narrowband cards support automatic SHO/HOO selection and independently
selected Automatic, RGB, narrowband or no stars. Source availability resolves
the automatic policy during preparation; the saved policy and resolved result
are separate. Source changes mark results out of date. Missing policy fields
preserve existing concrete palettes and narrowband stars.

The balanced setup stretches physical H-alpha/OIII/SII with H-alpha's resolved
Auto-MTF (target median 0.2, shadows −2.8), then balances OIII/SII against
H-alpha. Boosts start at 1 and shadow protection at 1. Foraxx retains its own
dynamic mapping and bypasses shared H-alpha stretching and balance. Display
balance never feeds linear photometry or continuum subtraction.

Balance can exclude circles around detected stars from its statistics. The
optional radius is 1–64 common-grid pixels; blank includes stars. Detection is
bounded to 4096 stars per physical plane. The union mask excludes samples and
pixel-difference noise pairs, without changing output coverage. Reports record
the detected-star count and excluded pixel count.

**Reduce green excess** starts at 0. On bounded display RGB it applies
`G' = G − amount × max(0, G − max(R, B))`, with amount in [0,1]. Red, blue and
neutral pixels are unchanged. This is an aesthetic operation with no SCNR
parity claim. It runs after mapping, before broadband structure and RGB stars.

An RGB-star branch optionally runs BXT/NXT on linear RGB, then the selected
Gaia calibration and SXT. The RGB original and starless copy share the same
color-preserving stretch before screen recombination. Gaia settings, its
pixel-solve/catalog basis and RC-Astro/model versions participate in the RGB
SXT cache identity. Disabling Gaia preserves the RGB processing branch.

## Subtract continuum for combined images

Combined cards are explicit recipes. Ordinary RGB, LRGB and narrowband cards
keep their saved behavior. The **Custom combination** selector chooses exact
source revisions independently across broadband and narrowband exposure groups.

| Recipe | Linear base and references | Result |
| --- | --- | --- |
| RGB + emission | R grid; R for Ha/SII, fitted G+B for OIII | Broadband colors plus validated signed residual contributions |
| LRGB + emission | L grid; same RGB references | RGB/emission blending followed by the existing L step |
| Narrowband + broadband structure | Ha grid; same RGB references | Mapped starless residuals, explicit starless broadband amount and optional RGB stars |

Natural-color amounts start at 0.25 per line. Positive contribution starts on
and changes only the display addition. Mapped broadband structure starts at
0. L supplies luminance, not a continuum reference. No G/B coefficient is fixed
at 50/50. These amounts are artistic choices, not line abundance measurements.

### Linear preparation and subtraction

1. Resolve exact sources and register them on the reference grid. Retain
   aligned, background-prepared linear samples before independent percentile
   scaling or any stretch.
2. Check original saturation evidence for every plane: a producer clipping map
   or a physical ceiling in the exact working units. Image maxima and display
   white points are not saturation evidence.
3. If explicitly enabled, measure common unsaturated stellar widths and broaden
   copies with normalized Gaussian kernels. Automatic target FWHM is 1.05 times
   the largest median width. A manual target must be at least every measured
   width, fit inside the aperture, and remain below two thirds of the sky
   annulus inner radius (8 pixels with default photometry; hard limit 32).
   No sharpening occurs. Propagate missing coverage
   across the complete kernel support, invalidate unsupported edges, and dilate
   clipping masks across that support.
4. Measure isolated local-background-subtracted aperture fluxes, using radii
   8/12/20 pixels. Search at most 4096 candidates independently of the 400 usable
   star budget. Reject ineligible stars before sampling across brightness.
   Fit a robust nonnegative through-origin model and audit its predictions
   against the full eligible pool.
5. Check held-out aperture residuals, spatial/brightness trends, registration,
   conditioning and profile widths. Independently compare held-out pixel
   profiles: sum absolute residuals above a three-sigma local noise allowance,
   divided by NB stellar flux. The 90th percentile must be at most 0.15. Core
   and halo signed fractions are reported separately so they cannot cancel
   inside the absolute metric. Require at least three validation profiles.
6. Subtract on those exact prepared samples. The plan records kernels and
   derived source revisions, and replays the same preparation when reused.
   Keep signed floating-point residuals and NaN coverage. Keep the separately
   propagated background offset out of the stored scientific residual.
7. For natural color, project residuals into reference units with the measured
   stellar reference ratios. Run color BXT/NXT on the broadband base, calibrate
   broadband RGB, apply its multiplicative gains to the added contribution,
   add L when present, then stretch the composition together. BXT/NXT on this
   lane precedes emission addition; it does not process the added residual.
8. For a mapped recipe, subtract before SXT, prepare starless branches, stretch
   and balance physical residuals, map the palette, reduce green if enabled,
   blend the selected broadband structure amount, and restore RGB stars once.

Matching is an explicit Gaussian approximation. Equal equivalent FWHMs do not
prove matched Moffat wings or cancellation of saturated stars. The independent
pixel check still applies. Original broadband star pixels keep their own BXT
option. Manual weights are explicitly unvalidated and require matching off in
the host; automatic results never silently become manual results.

Natural RGB/LRGB combinations retain broadband stars. A channel-card SXT step
therefore produces an actionable preparation error. Choose **Set in this
build** and remove SXT, or use a mapped recipe with RGB-star restoration. Mapped
recipes defer input SXT until after continuum subtraction.

### Outcomes and recovery

**Automatic** records each accepted or skipped line. A skipped natural-color
line is omitted and the broadband preview completes with warnings. A failed
required mapped line stops the new build and retains the previous preview.
**Manual** requires every coefficient; a blank optional OIII B/G ratio restores
measured photometry. **Off** adds no natural-color emission and preserves the
original narrowband signal in mapped recipes.

Per-line results show weights, eligibility, training/held-out counts, aperture
RMS, pixel validation and kernel resolution. Full JSON reports, signed residual
FITS, previews and a lazy stellar residual plot remain available for failed
builds as well as completed results.

### Calibrated line-flux export

A stellar fit alone produces an emission residual. Broadband filters can
transmit the same lines. Optional **Calibrated line-flux export** requires a
producer-supplied response calibration with exact sources, reference order,
revision, line labels and common flux unit. Responses include exposure, gain,
throughput, extinction and any doublet assumptions in the actual input units.
Filter names and widths cannot supply these values.

Seiza constructs `M[i,k] = NBresponse[i,k] − Σ a[i,j] BBresponse[i,j,k]`
and solves `M F = residuals − residualZeroLevels` for one to three coupled lines. It rejects
nonpositive or nearly cancelled diagonal response, rank/conditioning failures,
stale sources, inconsistent matching targets and numeric overflow. Row-scaled
conditioning and inversion preserve independent input gain units. Signed flux
and common coverage remain intact.

Optional residual covariance propagates as `M⁻¹ C M⁻ᵀ`. Missing covariance means
unknown uncertainty. Coefficient/response uncertainty, unmodelled lines and
spatial PSF errors are not included. Zero levels are explicit calibration inputs in residual sample units; the
server never guesses them from a sky median or display baseline. Their
uncertainty is also excluded unless supplied in the residual covariance.
The sidecar report retains the complete
calibration and plans. FITS declare the supplied BUNIT and calibrated-emission
product. This scientific export does not replace the artistic preview inputs.
Invalid requested calibration stops the build rather than exporting a partial
or falsely calibrated result.

### Jobs and cache identity

All image work runs in the existing tracked job with priority, worker leasing,
ComputePool and stop controls. Generated artifacts are written atomically below
the database slug. Registration and native channel tools remain reusable.

The aligned raw-linear cache includes sources, background treatment, transforms
and tools. Continuum cache identity includes exact source/mask revisions,
fit/photometry/matching controls, manual coefficients and the Seiza output
version. Amount changes reuse the fit and signed residual. Matching changes
invalidate prepared measurements and subtraction. Flux artifacts have a
separate identity containing every plan and calibration input. Completed-job
reuse requires all referenced reports, previews, residuals and flux files.

### Research and release boundary

The workflow is informed by [Cosmic Photons' process descriptions](https://cosmicphotons.com/scripts/)
and [NarrowbandNormalization documentation](https://www.cosmicphotons.com/pi-modules/narrowbandnormalization/nbnDocumentation20240816v010001.tar.gz),
and by [NightPhotons' stellar continuum workflow](https://www.nightphotons.com/software/photometric-continuum-subtraction/).
There is no third-party numerical-parity claim.
[Photutils PSF matching](https://photutils.readthedocs.io/en/latest/user_guide/psf_matching.html)
and [multi-wavelength matching research](https://arxiv.org/abs/1609.02006)
explain why Gaussian width matching alone does not establish profile equality.
[Continuum-removal calibration research](https://arxiv.org/abs/1110.5636)
explains the need for response calibration and stellar-color assumptions.

PSF Guard consumes published Seiza 0.43.0, stacking/output 0.50.0 and
satellites 0.22.1 through its registry-resolved lockfile. Local Cargo path
patches are only for paired development; do not commit a path-resolved
development lockfile as a registry release resolution.

Automatic continuum validation separately inspects isolated stellar candidates
excluded for clipping/saturation. They never train the fit. Every inspected
clipped profile must be measurable and below the same absolute residual limit;
reports show their count and worst residual. A clean unsaturated held-out sample
cannot conceal a clipped core or ring. This checks observed residuals and does
not reconstruct clipped flux.

### Reduce the restored RGB star layer

`NarrowbandProcessing.star_reduction` defaults to 0 and is omitted from stored
JSON at zero, preserving existing processing identities. A nonzero amount
changes the final composition identity, with linear inputs, BXT and SXT still
reusable. It applies only during RGB restoration, after mapping/green reduction
and broadband-structure blending. Native stars and natural RGB/LRGB are unchanged.

Seiza derives the display screen-star layer from the consistently stretched RGB
original/starless pair. Its maximum RGB component supplies an intensity plane.
Blend that plane toward a one-pixel cross erosion, then apply the resulting
common gain to all three star components before screen recombination. This
preserves layer RGB ratios and cannot brighten stars. Invalid neighbours and
outer edges keep the unreduced layer; centre coverage remains unchanged.
Zero follows the existing arithmetic bit for bit. This is display star-size/
prominence reduction, not a change to a calibrated PSF or scientific artifact.

### Real validation on Supergateway

The isolated validation registry used copied/binned completed Gecko Nebula
LRGB+SHO, Cocoon LRGB+SHO, Triangulum LRGB+Ha, NGC7023 LRGB and SNR G132
RGB/SHO masters. Fresh pixel solves succeeded, and Gaia calibrated the natural
RGB/LRGB outputs and the restored RGB-star branches. No original masters or
live catalog were modified. LRGB used real L masters.

The additional pixel gates intentionally change these automatic outcomes:

| Field / line | Pixel p90, automatic target | Pixel p90, explicit 8 px | Clipped-core maximum at 8 px |
| --- | --- | --- | --- |
| Gecko Ha | 27.0% | 3.9% | 50.7% |
| Gecko OIII | 29.6% | 1.0% | 48.4% |
| Gecko SII | 23.8% | 0.8% | 48.3% |
| Triangulum Ha | — | 4.3% | 41.7% |
| Cocoon Ha / OIII / SII | — | 4.5% / 2.4% / 2.2% | 30.0% / 58.8% / 69.6% |
| SNR G132 Ha | 22.0% | 6.1% | 84.0% |
| SNR G132 OIII | 30.1% | 2.0% | 78.4% |
| SNR G132 SII | 27.4% | 1.6% | 79.9% |

All these tested emission lines were omitted from natural-color previews;
required mapped builds stopped. Broader matching improved unsaturated profiles
without recovering clipped cores. The gallery labels these as safety outcomes,
not successful line enhancements. No calibrated response/zero-level data were
available, so no physical line-flux calibration was invented.

A separate clean SNR G132 star comparison keeps continuum Off and broadband
structure at 0. Licensed RC-Astro 2.6.6 ran BXT ML5 and SXT ML11 on CPU. BXT
used Sharpen Stars 0.5 and Adjust Star Halos 0 before Gaia/SXT. BXT off/on and
reduction 0/0.4 all completed on the same 4757×3156 crop. Reduction 0.4 reused
the exact BXT and SXT artifact IDs and completed in 8.11 seconds. Native
800×800 crops were rendered from display FITS with an identity transfer and
visually inspected. The offline Dropbox gallery includes the actual requests,
calibration/processing reports, checksums and display FITS.

Flux calibration also requires exact `continuumCoefficients` snapshots in each
plan's reference order. The server rejects a changed fit/manual coefficient
before exporting flux: retained source revisions do not establish that the
previous residual zero levels still apply. Update the explicit zero-level
calibration and coefficient snapshots when the subtraction model changes.
