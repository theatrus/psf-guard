# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Upgrading

- **Allow time for rebuilds.** The stacking engine changed, so each stack
  rebuilds once when next opened or refreshed.

## Changes

- Stack previews now run on the GPU when your computer has one: Metal on a
  Mac, Vulkan on Linux and Direct3D 12 on Windows. Debayering, star
  detection, resampling, the live stack and the final pass run there. On an
  Apple M4 Max, 35 frames of 26 megapixels stacked in 5–6 seconds instead of
  12. Computers without a supported GPU, and frames under about a million
  pixels, stack on the CPU as before. To choose which passes run on the GPU,
  open **Settings → Stacking → GPU**.
- In **Settings → PixInsight**, **Check again** now shows the executable and
  WBPP runs folder the server reports. A path you have edited but not saved
  stays as you typed it.
