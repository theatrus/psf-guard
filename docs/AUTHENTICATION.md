# Server authentication

PSF Guard can require a login when it runs as a web server. Authentication is
off by default. The Tauri desktop app does not use it: its server listens on
localhost and remains trusted by the desktop UI.

![PSF Guard server login](server-login.png)

## Configure viewer and editor accounts

Add one or both roles to the server TOML:

```toml
[server.auth]
session_hours = 168
secure_cookie = true
allow_read_only_compute = false

[server.auth.read_only]
username = "viewer"
password_file = "/run/secrets/psf-guard-viewer"

[server.auth.read_write]
username = "editor"
password_file = "/run/secrets/psf-guard-editor"
```

Restart `psf-guard server --config psf-guard.toml`. The browser will show a
PSF Guard login page. A successful login creates an HttpOnly, SameSite=Strict
session cookie. Sessions live in server memory, expire after `session_hours`,
and end when the server restarts.

Use `password_file` for deployed servers. The process trims leading and
trailing whitespace, so a final newline is safe. An inline `password` also
works for development:

```toml
[server.auth.read_write]
username = "editor"
password = "development-only-password"
```

Do not set both password fields for one role. Viewer and editor usernames must
differ. Secure cookies are the default. Set `secure_cookie = false` only for a
direct HTTP development server; a browser will not send a Secure cookie to a
plain HTTP URL.

## Roles

The viewer can read catalogs, images, quality results, exports, and cached
analysis. By default, the viewer cannot start costly stack builds, plate
solves, satellite predictions, or view-processing jobs. Cached results remain
available.

Set `allow_read_only_compute = true` to let the viewer start those derived-data
jobs. This can suit a trusted private server. Leave it off for a public demo,
where repeated jobs could consume CPU, network data, and cache space.

The editor can also:

- grade images and use undo or redo;
- update projects, targets, coordinates, and exposure plans;
- start quality scans and imports;
- manage databases, peers, catalogs, and calibration records when the matching
  server management gate is also enabled;
- write local exports and apply database sync previews.

Server checks enforce the role even if a client calls the API directly. The UI
also labels viewer sessions as **Read only**, hides Settings, and disables
grading controls.

## API sessions

Scripts that use the ordinary UI API can log in with a cookie jar:

```bash
curl -c psf-guard.cookies \
  -H "Content-Type: application/json" \
  -d '{"username":"editor","password":"..."}' \
  https://guard.example/api/auth/login

curl -b psf-guard.cookies https://guard.example/api/databases
```

Remote image upload and scheduler sync do not use this session. Their
`Authorization: Bearer ...` keys remain scoped to the configured database and
continue to work when browser authentication is enabled.

## Deployment notes

- Put the public server behind HTTPS. Login passwords otherwise cross the
  network in clear text.
- Restrict password-file permissions to the PSF Guard service account.
- On Windows, use a TOML literal string for paths with backslashes, such as
  `password_file = 'C:\ProgramData\PSF Guard\editor-password'`.
- Keep `--allow-database-management` off unless browser-side database
  management is needed. An editor login does not override that gate.
- Signing out revokes the current in-memory session. Restarting the server
  revokes every browser session.
