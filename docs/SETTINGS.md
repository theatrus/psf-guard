# Find a setting

Open **Settings** from the header menu. On a desktop, choose a page in the
grouped sidebar. On a phone, choose the same page from **Settings section**.
Only the selected page loads its controls.

| Group | Pages |
|---|---|
| **Databases & rigs** | **Databases**, **Rigs**, **Sync** |
| **Presets** | **Processing setups**, **Exposure templates** |
| **Processing** | **Calibration**, **Stacking** |
| **Performance** | **CPU & memory**, **GPU**, **Workers** |
| **Storage & backups** | **Folders & limits**, **Backups**, **Reference catalogs** |
| **Exports & integrations** | **Export defaults**, **PixInsight**, **AstroBin** |
| **Review** | **Grading & display** |
| **Server & access** | **Server**, **Users**, **Agents (MCP)** |

Pages depend on the connection and your permissions. Planning enables **Rigs**
and **Exposure templates**. **Sync** appears when you have a database and an
editor account. **Server** configures the desktop app's server connection.
The web interface shows **Users** and API tokens when server login is
enabled. Instances without accounts show **Agents (MCP)**. The desktop app
also offers its own worker controls under **Workers**.

## Find controls that moved

- **Setups** is now **Presets → Processing setups**. Your saved setups,
  default choices and setup names are preserved. The processing editors
  still offer **Apply setup** and **Save as…**.
- **Exposure templates** is under **Presets**. Templates remain reusable
  across rigs; moving the page does not change any rig's exposures.
- **GPU** moved from **Stacking** to **Performance → GPU**. GPU settings
  still apply to PSF Guard processing, including stack builds on workers.
- Processor shares and **Processing memory** are under **Performance →
  CPU & memory**. A worker computer still allocates its own CPU and memory;
  server resource limits do not become per-worker limits.
- Export defaults, PixInsight and AstroBin have their own pages under
  **Exports & integrations**.
- **Storage** is now **Storage & backups → Folders & limits**.
- **Catalogs** is now **Storage & backups → Reference catalogs**, for
  downloaded astronomy data such as solver indexes and Gaia colors.

**Databases** and **Rigs** remain separate pages in the same group.
Database settings control files and imports. Rig profiles control optics,
observing sites, scheduling and connections. Moving between these pages
does not change the database or rig configuration.

## Keep edits when navigating

When you choose another page in the sidebar or **Settings section** selector,
Settings asks before discarding unsaved form edits. Choose **Keep editing**
to return to the form. Choose **Discard changes and
switch** to open the requested page without saving those edits. **Close**
also asks before discarding unsaved changes.

Shortcuts from the Library, calibration reports, processing cards and Sky
still open the requested page, rig or catalog package. Saved settings keep
their existing scope and permissions: account preferences follow the
account, server settings apply to that server, and desktop worker settings
apply to the computer running the app.

## Save changes and close Settings

Each form has its own Save control, or applies a change immediately as its
help text explains. Click **Close** in the header to leave Settings. **Close**
does not save changes. Database results and unsaved-edit warnings appear
below the header when needed. The desktop app also offers **Open logs** in
the header.

Most saved settings apply immediately. These changes need a server or app
restart:

- **Server & access → Server:** after you save a different port, click
  **Restart server** to use it. The same control appears when the desktop
  server could not start. The restart waits for running work to finish,
  then reloads the interface. The general restart control has been removed
  from the other Settings pages.
- **Storage & backups → Folders & limits:** after you save a new cache,
  stack or calibration folder, click **Restart now** in the desktop app.
  This restarts the whole app so PSF Guard can move files before opening
  the catalogs. A browser user needs the server operator to stop and start
  PSF Guard. WBPP folder changes apply to new runs after saving.

Save or discard unsaved form edits before restarting. See
[Storage folders](STACKING_PREVIEWS.md#disk-use) for how
PSF Guard moves existing files.
