# Unreleased

> Add user-visible changes here as they merge. Before tagging a release, move
> these entries into `vVERSION.md`, adding to that file if it already exists.
> Then reset this file. See [the release guide](../RELEASING.md).

## Changes

- You can move a target to any project in its database, or to a new one.
  In Library, click **Edit** beside the target, then **Move to another
  project...**, choose the destination, and click **Preview move**. Its
  exposures, exposure plans and stack previews move with it, and a new
  project copies the current project's settings and starts Inactive. A target
  without exposures can also move to a project of another Target Scheduler
  profile; its exposure plans then use that profile's matching exposure
  templates, or copies of their own. A target with exposures stays within its
  profile, and a destination that already has a target of the same name
  refers you to **Merge target**. Before, **Move to:** moved the target at once,
  only within its profile, and left its stack previews behind.
- A restored reject returns to its target's current project, and a reject
  removed before **Merge target** returns to the destination target. Before,
  such a reject came back to its old project or to the merged-away target.
- **Merge target** removes the merged-away target's stack previews, since
  that target no longer exists.
- Editing a target's name together with an invalid value, such as an RA of
  25 hours, now changes nothing. Before, the new name was saved.

- You can reopen an archived project from the Library. Open **Archived
  projects** and click **Reopen** beside the project. PSF Guard sets it
  Inactive in Target Scheduler, so it returns to the project list but is not
  scheduled until you set it Active. A plan with nothing captured yet has
  **Reopen** while it is Closed on any rig; it sets the project Inactive on
  each of those rigs. Reopen appears for accounts that can edit.
- You can delete a project from a database. In the Library, open the
  project's **Edit project** panel and click **Delete project...**, or click
  **Delete** beside an archived project. The dialog
  shows how many targets, exposure plans and frames go, and asks you to type
  the project's name when it has frames. Frame records and grades leave the
  catalog; the image files stay on disk. Later syncs and imports leave a
  project with frames out until you click **Allow again** under **Deleted
  projects** in the Library; a project without frames is not listed there. A plan that linked the project no longer plans work for that
  rig. Deleting needs a server started with `--allow-database-management`.
  Restoring removed rejects no longer puts back a frame whose project was
  deleted.
- You can delete a plan with nothing captured. Under **Plans with nothing
  captured yet**, click the delete button beside the plan. The dialog lists
  the plan's project in each database; **Delete plan** deletes them and the
  plan with its framing. A plan whose rigs captured frames stays until you
  delete those projects from the Library.
- You can leave a collaboration. Open a plan imported from a collaboration,
  and under **Collaboration assignments** in Activation click **Leave
  collaboration**. The dialog lists what goes: the plan's open assignments in
  the rig databases end, its imported assignments are removed, unfinished
  file transfers for it are cancelled, and nightly intake stops importing the
  project. Reports not yet sent are deleted and never reach the coordinator;
  deliver them first with **Check in** to have them counted. Frames in
  reports lose their protection, so removing rejects and deleting the
  project in the Library can then remove them. PSF Guard declines the plan's
  tasks at the coordinator for each rig signed in to it; the dialog names
  any rig that is not, and you should leave or decline the project on the
  coordinator's site for that rig too. The plan stays as a plan of your own, with its framing, exposures,
  rig projects, frames and grades, so you can rename, attach, detach or
  delete it like any other plan. A database the plan links that is missing
  or cannot be written does not stop you leaving; the dialog warns that its
  assignments stay open until you register it again and activate the plan.
  Joining and importing the project again later rejoins the same plan.
- A plan from a collaboration with nothing captured can now be deleted under
  **Plans with nothing captured yet**. Deleting it leaves the collaboration
  first, as **Leave collaboration** does. A plan with frames in reports to the
  collaboration stays until you leave the collaboration from the plan.

- A color preview can run BlurXTerminator and NoiseXTerminator on its color
  channels together. In **Processing stack**, turn the tools on under
  **RC-Astro on the color channels together**. They run on the aligned channels before color
  calibration and the stretches. BlurXTerminator starts in **Correct only**
  mode, which corrects star shapes and color fringes without sharpening, and
  NoiseXTerminator reduces color noise. Changing a stretch or calibration
  afterward does not run the tools again. Every BlurXTerminator section now
  offers **Correct only**.
- A color card that cannot prepare a build now shows why, for example when
  RC-Astro is not installed or BlurXTerminator is not licensed on the server.
  The message appears beside **Apply processing stack** when the build came
  from there, and at the top of the card otherwise. Before, the card kept
  showing its previous preview without a message.

- BlurXTerminator, NoiseXTerminator and StarXTerminator no longer turn a
  stack upside down. RC-Astro read a stack without a row order, such as a WBPP
  master, as bottom-up and returned it flipped, so a color preview built from
  it misaligned that channel. Color previews that used RC-Astro show as out of
  date; build them again. A channel card that already shows a flipped result
  keeps it until you apply its processing again.

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
- A color preview now runs each channel's deconvolution and RC-Astro steps on
  the channel's own stack, before background extraction and alignment, and
  shares the results with the channel card. When a card and a color preview use
  the same settings on the same stack, BlurXTerminator, NoiseXTerminator or
  StarXTerminator runs once, and changing background extraction or a stretch no
  longer reruns them. A color preview that uses deconvolution or RC-Astro
  shows as out of date and runs its tools in the new order the next time you
  build it.
