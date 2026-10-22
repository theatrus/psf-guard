# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- A mono stack's card shows a processing run you started, such as
  BlurXTerminator, after you leave **Stacks** and come back. **Apply
  processing** shows the run's progress and stays disabled until it ends,
  and then the card shows the result. Before, the card showed no run while
  the background jobs list still showed it.
- A new color preview runs the deconvolution and RC-Astro steps each channel
  card applied. Apply BlurXTerminator or NoiseXTerminator to a channel's stack
  and the color preview built from it runs them on that input too, including
  automatic rebuilds after new frames. Each input lane in **Processing stack**
  names what its card applied, and a card that changes afterward makes the
  preview out of date. To set an input's tools in the color preview instead,
  choose **Set in this build** under **Deconvolution and RC-Astro**; it starts
  from what the cards show. Color previews made before this release keep their
  own settings.
- A stack's final pass, **Rejecting transients**, reads each frame once
  instead of three times and needs less memory: 45 frames of a 26 MP mono
  camera finish in about 9 seconds rather than 23, peaking at 5.7 GB rather
  than 9.9 GB. While it runs, the pass keeps a scratch copy of each frame in
  `stack-scratch` below the stack folder, four bytes per sample per frame,
  and removes it when it ends. The progress line counts two stages, reading
  and integrating, instead of three passes. When that disk is short of room,
  the pass reads every frame three times as before.
- Stacks move to Seiza stacking 0.44, so each stack rebuilds once the next time
  it is opened or refreshed. Mosaic backgrounds no longer copy the first
  frames' skies, and mosaics fit their lens more often.
- Colour calibration no longer tints clipped star cores cyan or blue. A stack
  records where its frames clipped, and calibration renders those pixels
  neutral, in one-shot-color stacks and in RGB, LRGB and narrowband
  compositions. Stacks built before this change calibrate as before until
  they rebuild.
