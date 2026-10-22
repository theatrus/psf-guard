# Color previews and channel processing

Status: **Step 1 built**: color previews follow each channel card's
deconvolution and RC-Astro steps by default. Step 2 is decided after a
comparison on a real stack, and comes in a later change. Step 3 is planned.
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

## Today's color pipeline

For each input, a color build runs, in order:

1. background extraction on the channel in its own frame, with one surface
   model shared by all channels;
2. registration onto the reference channel (R for RGB, L for LRGB, H-alpha
   for narrowband);
3. deconvolution and RC-Astro tools on the registered linear channel;
4. normalization and the input's stretch stages;
5. composition, then the RGB output stages.

A channel card runs its tools on the channel's own stack, before any of
this. Its results are filed under the stack (`mono:{job}:{group}`); a color
input's are filed under the color input (`color-input:{input}:{role}`), so
nothing is shared.

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

## Step 2: share each channel's tools (decided)

Decided: run each input's tools on the channel's own stack, before background
extraction and registration, sharing the channel card's cache. Implementation
comes in a later change.

One BlurXTerminator or NoiseXTerminator run then serves the card and every
color preview that uses that stack, and a change to background extraction no
longer reruns every channel's tools. Background extraction then fits
sharpened, denoised channels, and registration resamples sharpened stars.
Every color preview with tools rebuilds once after the change.

### The comparison

Teddy Bear on starfront-redcat61: the WBPP OIII master registered to the
H-alpha master after a synthetic 1.7° turn and a (23.4, −11.7) pixel shift,
measured on the central 2400×2400 pixels. RC-Astro ran through its CLI on
supergateway with default BlurXTerminator and NoiseXTerminator settings.
A registers and then runs the tools, as color previews do today; B runs the
tools and then registers.

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
