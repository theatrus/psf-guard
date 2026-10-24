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
- In the Settings sections **Setups → PixInsight**, **Stacking → Stack
  previews**, **Performance → Processor use** and, in the desktop app,
  **Server**, fields no longer appear blank, or show an Automatic port, for a
  moment while Settings loads. When
  the server reports new values, fields you have not edited update to match,
  and a field you are editing keeps what you typed. In PixInsight, **Check
  again** now updates the executable and WBPP runs folder.
- Settings no longer shows a second scrollbar when you open it from the
  Stacks page. Before, with a stack's **Exact measurements** table open,
  scrolling in Settings could also scroll the Stacks page behind it, and the
  page could be out of view when Settings closed.
