# Find a setting

Open **Settings** from the header menu. On a desktop, choose a page in the
grouped sidebar. On a phone, choose the same page from **Settings section**.
Only the selected page loads its controls.

| Group | Pages |
|---|---|
| **Databases & rigs** | **Overview**, each database and its pages, **New / import** |
| **Presets** | **Processing setups**, **Exposure templates**, **Sites** |
| **Processing** | **Calibration**, **Stacking** |
| **Performance** | **CPU & memory**, **GPU**, **Workers** |
| **Storage & backups** | **Folders & limits**, **Backups**, **Reference catalogs** |
| **Exports & integrations** | **Export defaults**, **PixInsight**, **AstroBin** |
| **Review** | **Grading & display** |
| **Server & access** | **Server**, **Users**, **Agents (MCP)** |

Pages depend on the connection and your permissions. Planning enables **Rig setup**
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

## Open a database

Choose a database by name in the left pane. Its name opens the database overview. **Rig setup**, **Database & files**
and **Sync** appear beneath the database name. On a phone, choose
the database in **Settings section**, then choose its page from the same
control. The selected page includes the database name.

- **Overview** shows the database and image-folder paths and links to its
  calibration library.
- **Rig setup** shows equipment, field of view, the selected observing site,
  scheduling limits and planning connections. Sites are shared: edit them
  under **Presets → Sites**, then choose a rig's **Planning site** in
  **Rig setup → Site**.
- **Database & files** contains paths, imports, scan rules, automatic import
  and database maintenance. Quality analysis, calibrated-copy pairing and
  AstroBin filter maps have moved to the pages described below.
- **Sync** configures the selected database's Target Scheduler connection,
  incoming uploads and pairing. Transfers use the opened database as their
  local endpoint. Peers and credentials remain shared.

The shared **Overview** lists databases and staged transfer previews. Each
preview acts on its recorded database and endpoints. Choose **New / import**
to create a database from image folders, import a local database, or connect
N.I.N.A. through PSF Guard Sync. Plugin onboarding opens the new database's
pairing controls directly. The pages use the same saved IDs and permissions.

For **Data Transfer**, the opened database receives **Merge databases** and
sends **Send planning** or **Send reviewed grades**. Choose the other endpoint,
check **Source** and **Destination**, then click **Preview changes** before
applying. Changing databases discards this form's obsolete preview without
retargeting unrelated staged previews or an applied job.

![Databases listed directly in the Settings navigation](images/rig-database-settings/rig-databases-desktop.png)

## Set shared defaults and database overrides

Open **Review → Grading & display → Quality analysis** for **Analyze new
frames as they arrive**, **Measure quality on the calibrated copy**, and
manual quality-analysis actions. Grading and scoring appear before quality
analysis; display preferences follow it.

Open **Processing → Calibration → Calibrated copies** for **Pair calibrated
and registered copies with their lights**. Shared defaults apply to new
databases. Existing databases keep their previous choices as explicit
overrides, including choices that matched the old built-in defaults.

Each page has **Shared defaults** and **Database overrides**. Expand a
database to review its values. Click **Override global setting** to set a
value for that database, then click **Save database overrides**. Turn
**Override global setting** off and save to use the shared value. A shared
change affects only databases that inherit that setting. It does not replace
another database's explicit choice. Save controls and unsaved-edit warnings
work separately for defaults and each database.

Turning automatic analysis on also queues frames already waiting for quality
analysis. Pairing changes apply to future imports and pairing scans. Changing
the quality source takes effect when quality analysis next measures a frame;
the cache still distinguishes raw and calibrated sources. Raw image files
remain unchanged.

Open **Exports & integrations → AstroBin** for shared filter IDs and the
**Database filter maps** below them. A database's matching filter/date entry
replaces the shared ID. The maps retain their existing scope and storage.

## Keep edits when navigating

When you choose another Settings page or database,
Settings asks before discarding unsaved form edits. Choose **Keep editing**
to return to the form. Choose **Discard changes and
switch** to open the requested page without saving those edits. **Close**
also asks before discarding unsaved changes.

Shortcuts from the Library, calibration reports, processing cards and Sky
still open the requested page, rig or catalog package. A shortcut without a
selected database opens the parent list first. If an explicitly selected
database is no longer registered, Settings shows that problem and lets you
return to the list; it does not select a different database. Saved settings keep
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
