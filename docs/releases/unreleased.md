# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

## Changed

## Fixed

- Changing one RC-Astro step no longer recomputes the whole chain. Each step
  caches under its own identity, so turning NoiseXTerminator on after a
  BlurXTerminator run — or retuning a later step — reuses every artifact
  before the change.
- Every view now works on a phone screen. The image detail view stacks the
  image above its info panel instead of squeezing the image to nothing, the
  comparison view keeps its Swap and Unmark buttons reachable, headers and
  overview project cards wrap instead of truncating, and keyboard cheat
  sheets stay out of the way on touch screens.
