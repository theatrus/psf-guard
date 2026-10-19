# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Fixes

- **Build stacks** and **Build current set** queue promptly on large
  projects. Matching each light to its calibration frames read every
  calibration file's header again for each light, so a project with a
  thousand lights took more than a minute to queue, longer while a quality
  scan ran, and a proxy in front of the server gave up without queueing
  anything. Each file is now read once per request, and the catalog stays
  free for other requests meanwhile.
- The uncalibrated-lights check, the calibration report, the AstroBin
  export and Export read the calibration library once rather than once per
  light or night. The check takes about a second on a large project, where
  it took about twenty, and Export no longer rescans every image folder
  before it starts.
- **Build stacks**, **Build current set** and **Build channel** join the
  queue the moment you click, as **Preparing stack build**, instead of
  waiting while the server reads every frame and matches calibration. The
  build then takes its place as one build per channel, and you can stop it
  while it is prepared.
- After a rebuild you ask for, **Re-apply processing when a stack
  refreshes** runs each channel's processing right after its build, ahead of
  your other queued builds. It used to wait at the back of the queue until
  every other build was done, and stepped aside whenever you started work.
- A stack card refreshed after an engine update shows its new preview as
  soon as the refresh finishes. The finished build used to read as done a
  moment before its new preview was recorded, so a card could keep the old
  one until the next refresh.
