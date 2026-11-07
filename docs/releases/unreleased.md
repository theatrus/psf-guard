# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- **Start Director without manual recovery:** Paired equipment reports apply automatically and preserve work permission and plan selection. With the matching Director plugin update, interrupted assignments retire automatically after the plugin confirms idle equipment. Saved images and receipts remain available, and fresh work uses current image counts.

- **Run a rig with the Director plugin:** Rig setup has a new **Director** tab. Generate a pairing code, see the equipment the plugin reports automatically, manage work permission and follow its work without calling the API.
- **Use Director and Sync on one rig:** Both plugins can stay active. Changing a rig's **Mode** keeps both pairings, the accepted equipment and the allowed work, so you can compare the two. Pairing Director no longer switches a rig set to Target Scheduler.
- **Run plans whose templates use the camera default:** Director uses the default gain and offset reported by N.I.N.A. for templates left at `-1`. Update PSF Guard Director and run the session to report those defaults automatically.
- **See when no plan can run:** The **Director** tab no longer says work is allowed when no activated plan can run. It lists why each plan was left out, and tells you how to start a session.
- **Trust a Director rig's night load:** The Nights page counts only the plans activated on a Director rig, and shows **Full** instead of large overbooked hours from targets that overlap.
- **See why a rig gets no work:** The **Director** tab explains when work is off, a plugin was removed or an activated plan cannot run. Equipment and setup updates preserve work permission.
- **Recover a stuck rig:** When a plugin is removed while it holds work, **Close work** lets the rig take work again. Attempts it reported stay spent.
- **Clearer pairing codes:** **Generate pairing code** is now **Generate Sync pairing code**, and the Sync tab shows where Director pairs.
