# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- You can start a plan with no images or database from **New plan** in the
  Library's toolbar or **New plan...** at the top of the header's project
  picker. **Create plan** opens the plan on **Framing** with **Object name**
  ready for the target. Before, **New plan** was only at the bottom of the
  Library, and saving left you in the list.
- **Rigs** and **Framing** show an **Add objective** button for a plan with
  no objectives. A rig joins a plan by shooting its objectives, so **Add a
  rig** and the rig switches on **Framing** stay disabled until the plan
  has one; before, only a tooltip and a grey line explained why. The new
  objective starts on a band that a rig or the template library has a
  template for, so a rig can join.
- **Download both** and `psf-guard-cli noise-model download` work again.
  Microsoft's GitHub repository for ONNX Runtime has been offline since
  about 18 November, so the download failed with a 404. PSF Guard now
  downloads a copy of the same ONNX Runtime 1.31.1 archive from its own
  GitHub release, checked against the same SHA-256 as before.
