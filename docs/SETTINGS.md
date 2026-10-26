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
enabled. Instances without accounts show **Agents (MCP)**. The desktop app also offers its own worker
controls under **Workers**.

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
switch** to open the requested page without saving those edits. **Done** and
the close button also ask before discarding unsaved changes.

Shortcuts from the Library, calibration reports, processing cards and Sky
still open the requested page, rig or catalog package. Saved settings keep
their existing scope and permissions: account preferences follow the
account, server settings apply to that server, and desktop worker settings
apply to the computer running the app.
