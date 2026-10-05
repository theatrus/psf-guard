# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- The desktop app writes a daily log file and keeps a week of them, on
  Windows, macOS and Linux. **Settings › Open logs** opens the folder. The log
  names each frame a stack leaves out or rejects, and why.

## Changed

- Dark masters use the nearest complete night of darks, and otherwise pool
  nights within reach, nearest first. Darks more than six months from the
  lights no longer match unless marked for that side of them. Both the reach
  and the size of a complete night are calibration settings.
- The calibration coverage report says why a night has no flats, naming the
  nearest flat refused for its rotator angle and how far off it is. Its dark
  column shows what one dark master would use instead of every matching dark,
  and it lists masters from other software with whether they are used and,
  if not, which readings disagree.
