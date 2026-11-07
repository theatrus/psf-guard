# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- **Start Director without manual recovery:** Paired equipment reports apply automatically and preserve work permission and plan selection. With the matching Director plugin update, interrupted assignments retire automatically after the plugin confirms idle equipment. Saved images and receipts remain available, and fresh work uses current image counts.

- **Run a rig with the Director plugin:** Rig setup has a new **Director** tab. Generate a pairing code, review the equipment the plugin reports, allow it to run the plans activated on the rig and follow its work without calling the API.
- **Use Director and Sync on one rig:** Both plugins can stay active. Changing a rig's **Mode** keeps both pairings, the accepted equipment and the allowed work, so you can compare the two. Pairing Director no longer switches a rig set to Target Scheduler.
- **Run plans whose templates use the camera default:** Director dropped every exposure template left at gain or offset `-1`, so a rig with those templates had nothing to run once you accepted its equipment. The plugin now reports N.I.N.A.'s default gain and offset, and Director uses them. Update PSF Guard Director, click **Report equipment**, accept the report, then click **Allow work again**.
- **See when no plan can run:** The **Director** tab no longer says work is allowed when no activated plan can run. It lists why each plan was left out, and tells you how to start a session.
- **Trust a Director rig's night load:** The Nights page counts only the plans activated on a Director rig, and shows **Full** instead of large overbooked hours from targets that overlap.
- **See why a rig gets no work:** The **Director** tab explains when a setup change, new equipment or a removed plugin stopped work, and **Allow work again** restarts it.
- **Recover a stuck rig:** When a plugin is removed while it holds work, **Close work** lets the rig take work again. Attempts it reported stay spent.
- **Clearer pairing codes:** **Generate pairing code** is now **Generate Sync pairing code**, and the Sync tab shows where Director pairs.
