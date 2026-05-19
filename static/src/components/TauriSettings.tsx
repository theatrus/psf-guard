import { useCallback, useEffect, useState } from 'react';
import { isTauriApp, tauriConfig, tauriFileSystem } from '../utils/tauri';
import type { DbEntry, DbRegistry } from '../utils/tauri';
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
 * Applies changes via the in-process Tauri commands directly, then asks the
 * embedded server to restart so the new registry is loaded.
 */
export default function TauriSettings({ isOpen, onClose }: TauriSettingsProps) {
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

  const reload = useCallback(async () => {
    setIsLoading(true);
    try {
      const reg = await tauriConfig.getCurrentConfiguration();
      setRegistry(reg);
      // If empty, default to showing the add form so the welcome flow lands
      // somewhere usable.
      setShowAddForm(!reg || reg.databases.length === 0);
    } catch (err) {
      console.error('Failed to load registry:', err);
    } finally {
      setIsLoading(false);
    }
  }, []);

  useEffect(() => {
    if (!isTauriApp() || !isOpen) return;
    reload();
    setStatusMessage('');
  }, [isOpen, reload]);

  const resetForm = () => {
    setEditingId(null);
    setFormName('');
    setFormDbPath('');
    setFormImageDirs([]);
    setShowAddForm(false);
  };

  const startEdit = (entry: DbEntry) => {
    setEditingId(entry.id);
    setFormName(entry.name);
    setFormDbPath(entry.db_path);
    setFormImageDirs(entry.image_dirs);
    setShowAddForm(true);
  };

  const startAdd = async () => {
    setEditingId(null);
    setFormName('');
    setFormImageDirs([]);
    setShowAddForm(true);

    // Try to seed with the default N.I.N.A. database path (Windows only).
    try {
      const def = await tauriFileSystem.getDefaultNinaPath();
      setFormDbPath(def || '');
    } catch {
      setFormDbPath('');
    }
  };

  const handlePickDbPath = async () => {
    try {
      const path = await tauriFileSystem.pickDatabaseFile();
      if (path) setFormDbPath(path);
    } catch (err) {
      console.error('pickDatabaseFile failed:', err);
    }
  };

  const handleAddImageDir = async () => {
    try {
      const path = await tauriFileSystem.pickImageDirectory();
      if (path && !formImageDirs.includes(path)) {
        setFormImageDirs([...formImageDirs, path]);
      }
    } catch (err) {
      console.error('pickImageDirectory failed:', err);
    }
  };

  const handleRemoveImageDir = (index: number) => {
    setFormImageDirs(formImageDirs.filter((_, i) => i !== index));
  };

  const handleSaveForm = async () => {
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
      if (editingId && registry) {
        // Edit-in-place: replace the matching entry in the registry and save.
        const updated: DbRegistry = {
          ...registry,
          databases: registry.databases.map((d) =>
            d.id === editingId
              ? {
                  ...d,
                  name: inferredName,
                  db_path: formDbPath.trim(),
                  image_dirs: formImageDirs,
                }
              : d
          ),
        };
        const ok = await tauriConfig.saveConfiguration(updated);
        if (!ok) throw new Error('saveConfiguration returned false');
      } else {
        const added = await tauriConfig.addDatabase(
          inferredName,
          formDbPath.trim(),
          formImageDirs
        );
        if (!added) throw new Error('addDatabase returned null');
      }

      await reload();
      resetForm();
      setStatusMessage('Saved.');
    } catch (err) {
      console.error('save failed:', err);
      setStatusMessage(`Failed to save: ${err}`);
    } finally {
      setIsApplying(false);
    }
  };

  const handleRemove = async (entry: DbEntry) => {
    if (!confirm(`Remove "${entry.name}" from the configured databases?`)) return;
    setIsApplying(true);
    try {
      const ok = await tauriConfig.removeDatabase(entry.id);
      if (ok) {
        await reload();
        setStatusMessage(`Removed ${entry.name}.`);
      } else {
        setStatusMessage('Remove failed.');
      }
    } catch (err) {
      console.error('remove failed:', err);
      setStatusMessage(`Failed to remove: ${err}`);
    } finally {
      setIsApplying(false);
    }
  };

  const handleApplyAndRestart = async () => {
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
          {!hasDatabases && (
            <div className="welcome-message">
              <h3>🚀 Welcome to PSF Guard Desktop!</h3>
              <p>Configure one or more N.I.N.A. scheduler databases to get started.</p>
            </div>
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
                </div>
                <div className="db-row-actions">
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
              </div>
            ))}

            {!showAddForm && (
              <button
                className="add-directory-button"
                onClick={startAdd}
                disabled={isApplying}
              >
                + Add Database
              </button>
            )}
          </div>

          {showAddForm && (
            <div className="settings-section">
              <h3>{editingId ? 'Edit Database' : 'Add Database'}</h3>

              <div className="database-config">
                <label>Display name (optional):</label>
                <input
                  type="text"
                  value={formName}
                  onChange={(e) => setFormName(e.target.value)}
                  placeholder="e.g. Imaging Rig (defaults to filename)"
                  className="file-path-input"
                />
              </div>

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

              <div className="database-config">
                <label>Image Directories:</label>
                <button onClick={handleAddImageDir} className="add-directory-button">
                  + Add Image Directory
                </button>
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
                  disabled={!formDbPath.trim() || isApplying}
                >
                  {editingId ? 'Save Changes' : 'Add Database'}
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
            <button onClick={onClose} className="cancel-button" disabled={isApplying}>
              Close
            </button>
            <button
              onClick={handleApplyAndRestart}
              className="save-button"
              disabled={isApplying || !hasDatabases}
              title="Restart the embedded server to pick up changes"
            >
              {isApplying ? 'Restarting…' : 'Apply & Restart Server'}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
