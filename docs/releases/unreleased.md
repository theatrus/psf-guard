# Unreleased

> Add a line here as each user-visible change merges. At release time this
> file becomes `vVERSION.md`; rewrite the opening sentence for the version
> being shipped and delete this note. See [the release guide](../RELEASING.md).

## Added

- `keep_failed_uploads` under `[server]` keeps a rejected remote upload's
  staged file in the receive directory, and names it in the log, so what a
  client sent can be examined. Off by default, a failed upload leaves nothing
  behind.

## Changed

## Fixed

- Remote image uploads now take the receive directory's usual file
  permissions (umask or default ACL) instead of a temporary file's
  owner-only `0600`, so other tools on the server can read what arrived.

- Browser accounts can sign in over a same-origin direct HTTP connection when
  `secure_cookie` is omitted, while HTTPS and untrusted requests remain
  Secure. Direct HTTP sign-in shows a cleartext warning, failed cookie
  persistence produces a useful error, and local servers on different ports
  no longer overwrite each other's sessions.
