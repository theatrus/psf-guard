# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

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
- The noise model shows as ready as soon as its download ends. Before, the
  page could keep showing it as not installed until you reopened it.
