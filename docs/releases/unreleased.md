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
