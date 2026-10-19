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
