# Color previews and channel processing

Status: **Steps 1 and 2 built**: color previews follow each channel card's
deconvolution and RC-Astro steps by default, and run them on each channel's
own stack in the card's cache. Step 3 is planned.
Last updated: 2026-10-22

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
3. RC-Astro tools can run on the composed color image.

## The color pipeline

For each input, a color build runs, in order:

1. registration onto the reference channel (R for RGB, L for LRGB, H-alpha
   for narrowband), measured on the stacks as they came;
2. deconvolution and RC-Astro tools on the channel's own stack, in its
   channel card's cache (step 2);
3. background extraction on the channel in its own frame, with one surface
   model shared by all channels;
4. resampling onto the reference with the transform from step 1;
5. normalization and the input's stretch stages;
6. composition, then the RGB output stages.

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
  stack"); the next build runs them in the new order once.
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

## Step 3: RC-Astro on the composed image (planned)

An optional RC-Astro stage on the linear RGB composition, before the output
stretch. BlurXTerminator's correct-only mode fixes star colour fringes left
by channel registration or the optics, and NoiseXTerminator on the color
image also reduces color noise, which per-channel runs cannot. It is a stage
of its own, so a build can run a tool per channel, on the composition, or
both.
