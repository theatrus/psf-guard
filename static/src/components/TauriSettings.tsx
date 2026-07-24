import { useCallback, useEffect, useState } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { isTauriApp, tauriConfig, tauriFileSystem } from '../utils/tauri';
import type { DbEntry, DbRegistry } from '../utils/tauri';
import { apiClient } from '../api/client';
import type { DatabaseSummary } from '../api/types';
import { describeImportProgress, useImportJob } from '../hooks/useImportJob';
import QualityBackfillControls from './QualityBackfillControls';
import SchedulerSyncControls from './SchedulerSyncControls';
import SeizaCatalogControls from './SeizaCatalogControls';
import './TauriSettings.css';

interface TauriSettingsProps {
  isOpen: boolean;
  onClose: () => void;
}

/**
 * Multi-database settings modal.
 *
 * Lists every configured database and lets the user edit name / image_dirs in
 * place, remove a database, or add a new one. Slug renaming is intentionally
 * not exposed here (breaks every existing bookmark for that DB) — users who
 * really want to rename a slug can hand-edit `config.json`.
 *
 * Works in both Tauri and browser/CLI-server mode:
 * - Tauri mode prefers the in-process commands so add/edit feel native and
 *   file pickers open OS dialogs.
 * - Browser mode falls back to HTTP `POST/PUT/DELETE /api/databases` so the
 *   same UI is usable when the server was launched via `psf-guard server`.
 *   The file pickers degrade to plain text inputs.
 */
export default function TauriSettings({ isOpen, onClose }: TauriSettingsProps) {
  const isTauri = isTauriApp();
  const queryClient = useQueryClient();
  const { data: serverInfo } = useQuery({
    queryKey: ['serverInfo'],
    queryFn: apiClient.getServerInfo,
    staleTime: 5 * 60 * 1000,
  });
  // CRUD requires either Tauri (in-process commands always allowed) or the
  // CLI server having been launched with --allow-database-management. The
  // gate is enforced server-side; we mirror it here to hide UI that would
  // just 403.
  const managementAllowed = isTauri || (serverInfo?.allow_database_management ?? false);
  const [registry, setRegistry] = useState<DbRegistry | null>(null);
  const [isLoading, setIsLoading] = useState(false);
  const [isApplying, setIsApplying] = useState(false);
  const [statusMessage, setStatusMessage] = useState<string>('');

  // Inline edit/add form state.
  const [editingId, setEditingId] = useState<string | null>(null); // null = add, slug = edit
  const [formName, setFormName] = useState('');
  const [formDbPath, setFormDbPath] = useState('');
  const [formImageDirs, setFormImageDirs] = useState<string[]>([]);
  const [showAddForm, setShowAddForm] = useState(false);
  // true = "create a brand-new TS database from image folders" flow (no
  // existing .sqlite required; the server bootstraps the full TS schema).
  const [createMode, setCreateMode] = useState(false);
  const [createAnalyzeQuality, setCreateAnalyzeQuality] = useState(false);
  const [importAnalyzeQuality, setImportAnalyzeQuality] = useState(false);

  // Slug of the database whose import job we're currently tracking; drives
  // the 1s progress poll + the progress panel at the bottom of the modal.
  const [importDbId, setImportDbId] = useState<string | null>(null);
  const { progress: importProgress, isRunning: importRunning } = useImportJob(importDbId);
  // A running preview survives closing or reloading this page. Keep the
  // destination so its completed dry-run can still show the confirm step.
  const [confirmImport, setConfirmImport] = useState<DbEntry | null>(null);

  const reload = useCallback(async () => {
    setIsLoading(true);
    try {
      // Prefer the Tauri command (returns the full registry including
      // schema_version and active_db_id); fall back to the HTTP listing
      // which gives us enough to render the UI.
      let reg: DbRegistry | null = null;
      if (isTauri) {
        reg = await tauriConfig.getCurrentConfiguration();
      }
      if (!reg) {
        const summaries: DatabaseSummary[] = await apiClient.getDatabases();
        reg = {
          schema_version: 2,
          databases: summaries.map((s) => ({
            id: s.id,
            name: s.name,
            db_path: s.database_path,
            image_dirs: s.image_directories,
          })),
        };
      }
      setRegistry(reg);

      // Import jobs live on the server, while importDbId is only view state.
      // Recover a running job when settings opens so progress polling resumes
      // after a page reload. There can be one job per database; this modal
      // shows the first active job in registry order.
      const runningImport = (
        await Promise.all(
          reg.databases.map(async (entry) => {
            try {
              const status = await apiClient.getImportStatus(entry.id);
              return status.progress.running ? entry : null;
            } catch (err) {
              console.warn(`Failed to check import status for ${entry.id}:`, err);
              return null;
            }
          })
        )
      ).find((entry): entry is DbEntry => entry !== null);
      if (runningImport) {
        setImportDbId(runningImport.id);
        setConfirmImport(runningImport);
      }

      // If empty AND we're allowed to mutate, default to showing the add
      // form so the welcome flow lands somewhere usable.
      setShowAddForm((!reg || reg.databases.length === 0) && managementAllowed);
    } catch (err) {
      console.error('Failed to load registry:', err);
    } finally {
      setIsLoading(false);
    }
  }, [isTauri, managementAllowed]);

  useEffect(() => {
    if (!isOpen) return;
    reload();
    setStatusMessage('');
  }, [isOpen, reload]);

  const resetForm = () => {
    setEditingId(null);
    setFormName('');
    setFormDbPath('');
    setFormImageDirs([]);
    setShowAddForm(false);
    setCreateMode(false);
    setCreateAnalyzeQuality(false);
  };

  const startEdit = (entry: DbEntry) => {
    setEditingId(entry.id);
    setFormName(entry.name);
    setFormDbPath(entry.db_path);
    setFormImageDirs(entry.image_dirs);
    setShowAddForm(true);
    setCreateMode(false);
  };

  const startCreate = () => {
    setEditingId(null);
    setFormName('');
    setFormDbPath('');
    setFormImageDirs([]);
    setShowAddForm(true);
    setCreateMode(true);
    setCreateAnalyzeQuality(false);
  };

  const startAdd = async () => {
    setEditingId(null);
    setFormName('');
    setFormImageDirs([]);
    setShowAddForm(true);
    setCreateMode(false);
    setFormDbPath('');

    if (isTauri) {
      // Try to seed with the default N.I.N.A. database path (Windows only).
      try {
        const def = await tauriFileSystem.getDefaultNinaPath();
        if (def) setFormDbPath(def);
      } catch {
        // Ignore — the form just stays empty.
      }
    }
  };

  const handlePickDbPath = async () => {
    if (!isTauri) {
      setStatusMessage(
        'File picker is only available in the desktop app — paste the path into the field.'
      );
      return;
    }
    try {
      const path = await tauriFileSystem.pickDatabaseFile();
      if (path) setFormDbPath(path);
    } catch (err) {
      console.error('pickDatabaseFile failed:', err);
    }
  };

  const handleAddImageDir = async () => {
    if (!isTauri) {
      setStatusMessage(
        'Image directory picker is only available in the desktop app — type the path below and press the Add button.'
      );
      return;
    }
    try {
      const path = await tauriFileSystem.pickImageDirectory();
      if (path && !formImageDirs.includes(path)) {
        setFormImageDirs([...formImageDirs, path]);
      }
    } catch (err) {
      console.error('pickImageDirectory failed:', err);
    }
  };

  // Browser-mode fallback: manually add an image directory from a text input.
  const [pendingImageDir, setPendingImageDir] = useState('');
  const handleAddManualImageDir = () => {
    const trimmed = pendingImageDir.trim();
    if (trimmed && !formImageDirs.includes(trimmed)) {
      setFormImageDirs([...formImageDirs, trimmed]);
      setPendingImageDir('');
    }
  };

  const handleRemoveImageDir = (index: number) => {
    setFormImageDirs(formImageDirs.filter((_, i) => i !== index));
  };

  const handleSaveForm = async () => {
    if (createMode) {
      // "New database from images": the server bootstraps a fresh TS-schema
      // database and imports the folders in the background.
      if (formImageDirs.length === 0) {
        setStatusMessage('Add at least one image directory to import');
        return;
      }
      const name = formName.trim() || 'Imported Images';
      setIsApplying(true);
      setStatusMessage('');
      try {
        const created = await apiClient.createDatabaseFromImages({
          name,
          image_dirs: formImageDirs,
          backfill: createAnalyzeQuality,
        });
        queryClient.invalidateQueries({ queryKey: ['databases'] });
        queryClient.invalidateQueries({ queryKey: ['db'] });
        setImportDbId(created.database.id);
        await reload();
        resetForm();
        setStatusMessage(`Created ${created.database.name}; importing images…`);
      } catch (err) {
        console.error('create-from-images failed:', err);
        const msg = err instanceof Error ? err.message : String(err);
        setStatusMessage(`Failed to create: ${msg}`);
      } finally {
        setIsApplying(false);
      }
      return;
    }

    if (!formDbPath.trim()) {
      setStatusMessage('Please select a database file');
      return;
    }

    const inferredName =
      formName.trim() ||
      formDbPath.split(/[\\/]/).pop()?.replace(/\.[^.]+$/, '') ||
      'Database';

    setIsApplying(true);
    setStatusMessage('');

    try {
      // Use HTTP endpoints — they're available in both Tauri and CLI-server
      // mode, and updating live `AppState.databases` rather than waiting for
      // a server restart.
      if (editingId) {
        await apiClient.updateDatabase(editingId, {
          name: inferredName,
          db_path: formDbPath.trim(),
          image_dirs: formImageDirs,
        });
      } else {
        await apiClient.addDatabase({
          name: inferredName,
          db_path: formDbPath.trim(),
          image_dirs: formImageDirs,
        });
      }

      // Invalidate every per-DB query so the merged-overview hooks pull
      // fresh data for the just-added/edited DB.
      queryClient.invalidateQueries({ queryKey: ['databases'] });
      queryClient.invalidateQueries({ queryKey: ['db'] });

      await reload();
      resetForm();
      setStatusMessage('Saved.');
    } catch (err) {
      console.error('save failed:', err);
      const msg = err instanceof Error ? err.message : String(err);
      setStatusMessage(`Failed to save: ${msg}`);
    } finally {
      setIsApplying(false);
    }
  };

  // Import is two-step: a dry-run PREVIEW first (rolled back server-side),
  // then an explicit confirmation. Nothing touches the database until the
  // user has seen exactly what would be attached vs newly created.
  const handleImport = async (entry: DbEntry) => {
    if (entry.image_dirs.length === 0) {
      setStatusMessage(
        `"${entry.name}" has no image directories configured — edit it and add the folders to import.`
      );
      return;
    }
    setIsApplying(true);
    setStatusMessage('');
    setImportAnalyzeQuality(false);
    try {
      const status = await apiClient.startImport(entry.id, { dry_run: true, backfill: false });
      setImportDbId(entry.id);
      setConfirmImport(entry);
      setStatusMessage(
        status.started
          ? `Previewing import into ${entry.name}… nothing is written until you confirm.`
          : 'An import is already running for this database.'
      );
    } catch (err) {
      console.error('import preview failed to start:', err);
      const msg = err instanceof Error ? err.message : String(err);
      setStatusMessage(`Failed to preview import: ${msg}`);
    } finally {
      setIsApplying(false);
    }
  };

  const handleConfirmImport = async () => {
    if (!confirmImport) return;
    const entry = confirmImport;
    setConfirmImport(null);
    setIsApplying(true);
    try {
      await apiClient.startImport(entry.id, {
        dry_run: false,
        backfill: importAnalyzeQuality,
      });
      setStatusMessage(`Importing images into ${entry.name}…`);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      setStatusMessage(`Failed to start import: ${msg}`);
    } finally {
      setIsApplying(false);
    }
  };

  const handleRemove = async (entry: DbEntry) => {
    if (!confirm(`Remove "${entry.name}" from the configured databases?`)) return;
    setIsApplying(true);
    try {
      const ok = await apiClient.removeDatabase(entry.id);
      if (ok) {
        queryClient.invalidateQueries({ queryKey: ['databases'] });
        queryClient.invalidateQueries({ queryKey: ['db', entry.id] });
        await reload();
        setStatusMessage(`Removed ${entry.name}.`);
      } else {
        setStatusMessage('Remove failed.');
      }
    } catch (err) {
      console.error('remove failed:', err);
      const msg = err instanceof Error ? err.message : String(err);
      setStatusMessage(`Failed to remove: ${msg}`);
    } finally {
      setIsApplying(false);
    }
  };

  // CRUD changes are applied to the live server immediately (HTTP endpoints
  // update both registry file and AppState.databases). The restart button is
  // only useful in rare cases where the live state diverged from disk — keep
  // it as an opt-in escape hatch in Tauri mode.
  const handleRestart = async () => {
    if (!isTauri) {
      setStatusMessage('Refreshing interface...');
      setTimeout(() => window.location.reload(), 800);
      return;
    }
    setIsApplying(true);
    setStatusMessage('Restarting server...');
    try {
      const restarted = await tauriConfig.restartServer();
      if (restarted) {
        setStatusMessage('Restarting interface...');
        setTimeout(() => window.location.reload(), 1500);
      } else {
        setStatusMessage('Server restart failed; falling back to app restart...');
        setTimeout(() => tauriConfig.restartApplication(), 1500);
      }
    } catch (err) {
      console.error('restart failed:', err);
      setStatusMessage(`Restart failed: ${err}`);
      setIsApplying(false);
    }
  };

  if (!isOpen) return null;

  const databases = registry?.databases ?? [];
  const hasDatabases = databases.length > 0;

  return (
    <div className="tauri-settings modal-overlay" onClick={onClose}>
      <div className="modal-content" onClick={(e) => e.stopPropagation()}>
        <div className="modal-header">
          <h2>PSF Guard Settings</h2>
          <button className="close-button" onClick={onClose}>×</button>
        </div>

        <div className="modal-body">
          {!hasDatabases && managementAllowed && (
            <div className="welcome-message">
              <h3>🚀 Welcome to PSF Guard!</h3>
              <p>Configure one or more N.I.N.A. scheduler databases to get started.</p>
            </div>
          )}

          {!managementAllowed && (
            <div className="welcome-message" style={{ borderColor: 'var(--color-border-warning, #c62)' }}>
              <h3>🔒 Database management is read-only</h3>
              <p>
                This server was launched without
                <code style={{ margin: '0 4px' }}>--allow-database-management</code>,
                so the configured database list cannot be changed from the
                browser. Add databases on the command line —
                <code style={{ margin: '0 4px' }}>
                  psf-guard server &lt;db&gt; &lt;image-dirs…&gt;
                </code>
                — or restart the server with the flag to enable add/edit/remove
                here.
              </p>
            </div>
          )}

          {managementAllowed && <SeizaCatalogControls />}

          {managementAllowed && (
            <SchedulerSyncControls databases={databases} disabled={isApplying} />
          )}

          <div className="settings-section">
            <h3>Configured Databases {hasDatabases && <span className="muted">({databases.length})</span>}</h3>

            {isLoading && <div className="detecting-database">Loading…</div>}

            {!isLoading && databases.length === 0 && !showAddForm && (
              <div className="no-directories">
                No databases configured yet.
              </div>
            )}

            {databases.map((entry) => (
              <div key={entry.id} className="db-row">
                <div className="db-row-main">
                  <div className="db-row-title">
                    <strong>{entry.name}</strong>{' '}
                    <code className="db-row-slug">{entry.id}</code>
                  </div>
                  <div className="path-info">{entry.db_path}</div>
                  {entry.image_dirs.length > 0 && (
                    <div className="path-info muted">
                      {entry.image_dirs.join(', ')}
                    </div>
                  )}
                  <QualityBackfillControls dbId={entry.id} />
                </div>
                {managementAllowed && (
                  <div className="db-row-actions">
                    <button
                      className="browse-button"
                      onClick={() => handleImport(entry)}
                      disabled={isApplying || (importRunning && importDbId === entry.id)}
                      title="Scan this database's image directories and import new FITS frames"
                    >
                      {importRunning && importDbId === entry.id ? 'Importing…' : 'Import'}
                    </button>
                    <button
                      className="browse-button"
                      onClick={() => startEdit(entry)}
                      disabled={isApplying}
                    >
                      Edit
                    </button>
                    <button
                      className="remove-button"
                      onClick={() => handleRemove(entry)}
                      disabled={isApplying}
                      title="Remove this database"
                    >
                      Remove
                    </button>
                  </div>
                )}
              </div>
            ))}

            {managementAllowed && !showAddForm && (
              <div className="db-add-buttons">
                <button
                  className="add-directory-button"
                  onClick={startAdd}
                  disabled={isApplying}
                >
                  + Add Database
                </button>
                <button
                  className="add-directory-button"
                  onClick={startCreate}
                  disabled={isApplying}
                  title="Create a brand-new Target Scheduler database and import folders of FITS images into it"
                >
                  ✨ New Database from Images
                </button>
              </div>
            )}

            {importDbId && importProgress && importProgress.stage !== '' && (
              <div className="import-progress-panel">
                <div className="import-progress-line">
                  {importRunning && <span className="import-spinner">⏳ </span>}
                  {describeImportProgress(importProgress)}
                </div>
                {importProgress.stage === 'complete' && importProgress.outcome && (
                  <>
                    {importProgress.outcome.attach_summaries.length > 0 && (
                      <ul className="import-project-list">
                        {importProgress.outcome.attach_summaries.map((a) => (
                          <li key={`${a.project}:${a.target}`}>
                            ↳ existing {a.project} / {a.target} — +{a.frames} frame(s) (
                            {a.matched_by} match)
                          </li>
                        ))}
                      </ul>
                    )}
                    {importProgress.outcome.project_summaries.length > 0 && (
                      <ul className="import-project-list">
                        {importProgress.outcome.project_summaries.map((p) => (
                          <li key={p.name}>
                            NEW {p.name} — {p.targets} target(s), {p.frames} frame(s)
                          </li>
                        ))}
                      </ul>
                    )}
                    {importProgress.outcome.dry_run &&
                      confirmImport &&
                      importDbId === confirmImport.id && (
                        <div className="modal-buttons import-confirm-buttons">
                          {importProgress.outcome.imported > 0 ? (
                            <div className="import-confirm-content">
                              <label className="quality-analysis-option">
                                <input
                                  type="checkbox"
                                  checked={importAnalyzeQuality}
                                  onChange={(event) =>
                                    setImportAnalyzeQuality(event.target.checked)
                                  }
                                />
                                <span>
                                  <strong>Queue background quality analysis</strong>
                                  <small>
                                    Reads every image to measure stars, background, clouds,
                                    obstructions, and pointing. This can take a long time,
                                    especially in a debug build. You can run it later from this
                                    database&apos;s settings.
                                  </small>
                                </span>
                              </label>
                              <div className="modal-buttons import-action-buttons">
                              <button
                                className="save-button"
                                onClick={handleConfirmImport}
                                disabled={isApplying}
                              >
                                Import {importProgress.outcome.imported} frame(s)
                              </button>
                              <button
                                className="cancel-button"
                                onClick={() => {
                                  setConfirmImport(null);
                                  setStatusMessage('Import cancelled — nothing was written.');
                                }}
                                disabled={isApplying}
                              >
                                Cancel
                              </button>
                              </div>
                            </div>
                          ) : (
                            <span className="muted">
                              Nothing new to import — every frame is already in the database.
                            </span>
                          )}
                        </div>
                      )}
                  </>
                )}
              </div>
            )}
          </div>

          {managementAllowed && showAddForm && (
            <div className="settings-section">
              <h3>
                {createMode
                  ? 'New Database from Images'
                  : editingId
                    ? 'Edit Database'
                    : 'Add Database'}
              </h3>

              {createMode && (
                <p className="muted">
                  Creates a brand-new Target Scheduler database and imports the
                  selected folders. Each target gets its own project. Nearby,
                  similarly dated panels with matching panel names share a
                  mosaic project. You can rename or reorganize them afterwards.
                  The import reads headers only; pixel-based quality work is a
                  separate option below.
                </p>
              )}

              <div className="database-config">
                <label>Display name (optional):</label>
                <input
                  type="text"
                  value={formName}
                  onChange={(e) => setFormName(e.target.value)}
                  placeholder={
                    createMode
                      ? 'e.g. 2026 Archive (defaults to "Imported Images")'
                      : 'e.g. Imaging Rig (defaults to filename)'
                  }
                  className="file-path-input"
                />
              </div>

              {!createMode && (
                <div className="database-config">
                  <label>N.I.N.A. Database File:</label>
                  <div className="file-input-group">
                    <input
                      type="text"
                      value={formDbPath}
                      onChange={(e) => setFormDbPath(e.target.value)}
                      placeholder="Select or enter database path"
                      className="file-path-input"
                    />
                    <button onClick={handlePickDbPath} className="browse-button">
                      Browse…
                    </button>
                  </div>
                </div>
              )}

              <div className="database-config">
                <label>Image Directories:</label>
                {isTauri ? (
                  <button onClick={handleAddImageDir} className="add-directory-button">
                    + Add Image Directory
                  </button>
                ) : (
                  <div className="file-input-group">
                    <input
                      type="text"
                      value={pendingImageDir}
                      onChange={(e) => setPendingImageDir(e.target.value)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') {
                          e.preventDefault();
                          handleAddManualImageDir();
                        }
                      }}
                      placeholder="Type an absolute path and press Add"
                      className="file-path-input"
                    />
                    <button
                      onClick={handleAddManualImageDir}
                      className="browse-button"
                      disabled={!pendingImageDir.trim()}
                    >
                      Add
                    </button>
                  </div>
                )}
                {formImageDirs.length > 0 && (
                  <div className="image-directories">
                    {formImageDirs.map((dir, index) => (
                      <div key={dir} className="image-directory-item">
                        <span>📂 {dir}</span>
                        <button
                          onClick={() => handleRemoveImageDir(index)}
                          className="remove-button"
                          title="Remove directory"
                        >
                          ×
                        </button>
                      </div>
                    ))}
                  </div>
                )}
              </div>

              {createMode && (
                <label className="quality-analysis-option">
                  <input
                    type="checkbox"
                    checked={createAnalyzeQuality}
                    onChange={(event) => setCreateAnalyzeQuality(event.target.checked)}
                  />
                  <span>
                    <strong>Queue background quality analysis</strong>
                    <small>
                      Reads every image to measure stars, background, clouds, obstructions,
                      and pointing. This can take a long time, especially in a debug build.
                      You can run it later from this database&apos;s settings.
                    </small>
                  </span>
                </label>
              )}

              <div className="modal-buttons">
                <button
                  onClick={resetForm}
                  className="cancel-button"
                  disabled={isApplying}
                >
                  Cancel
                </button>
                <button
                  onClick={handleSaveForm}
                  className="save-button"
                  disabled={
                    (createMode ? formImageDirs.length === 0 : !formDbPath.trim()) || isApplying
                  }
                >
                  {createMode
                    ? 'Create & Import'
                    : editingId
                      ? 'Save Changes'
                      : 'Add Database'}
                </button>
              </div>
            </div>
          )}
        </div>

        <div className="modal-footer">
          {statusMessage && (
            <div
              className={`save-message ${
                statusMessage.includes('Failed') || statusMessage.includes('failed')
                  ? 'error'
                  : 'success'
              }`}
            >
              {statusMessage}
            </div>
          )}
          <div className="modal-buttons">
            <button onClick={onClose} className="save-button" disabled={isApplying}>
              Done
            </button>
            <button
              onClick={handleRestart}
              className="cancel-button"
              disabled={isApplying}
              title={
                isTauri
                  ? 'Force a server restart (rarely needed — changes are applied live)'
                  : 'Reload the page'
              }
            >
              {isTauri ? 'Restart Server' : 'Reload Page'}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
