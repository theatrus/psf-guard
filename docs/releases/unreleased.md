# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- **Run a rig with the Director plugin:** Rig setup has a new **Director** tab. Generate a pairing code, review the equipment the plugin reports, allow it to run the plans activated on the rig and follow its work without calling the API.
- **Use Director and Sync on one rig:** Both plugins can stay active. Changing a rig's **Mode** keeps both pairings, the accepted equipment and the allowed work, so you can compare the two. Pairing Director no longer switches a rig set to Target Scheduler.
- **See why a rig gets no work:** The **Director** tab explains when a setup change, new equipment or a removed plugin stopped work, and **Allow work again** restarts it.
- **Recover a stuck rig:** When a plugin is removed while it holds work, **Close work** lets the rig take work again. Attempts it reported stay spent.
- **Clearer pairing codes:** **Generate pairing code** is now **Generate Sync pairing code**, and the Sync tab shows where Director pairs.
