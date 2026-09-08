# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- Merge alternate target names in Overview and move selected exposures to an
  existing or new target or project from Grid, with a preview before Apply.
  Image files and grades stay unchanged, and Target Scheduler plan counts
  follow the new grouping.

## Changed

## Fixed

- Browser accounts can sign in over a same-origin direct HTTP connection when
  `secure_cookie` is omitted, while HTTPS and untrusted requests remain
  Secure. Direct HTTP sign-in shows a cleartext warning, failed cookie
  persistence produces a useful error, and local servers on different ports
  no longer overwrite each other's sessions.
