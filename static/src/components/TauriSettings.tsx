import { useState, useEffect } from 'react';
import { isTauriApp, tauriFileSystem, tauriConfig } from '../utils/tauri';
import './TauriSettings.css';

interface TauriSettingsProps {
  isOpen: boolean;
  onClose: () => void;
}

export default function TauriSettings({ isOpen, onClose }: TauriSettingsProps) {
  // Band-aid form for B3: lets the user add ONE database to the registry.
  // F3 will replace this with the full multi-DB management panel.
  const [databaseName, setDatabaseName] = useState<string>('');
  const [databasePath, setDatabasePath] = useState<string>('');
  const [imageDirs, setImageDirs] = useState<string[]>([]);
  const [isDetectingDatabase, setIsDetectingDatabase] = useState(false);
  const [isSaving, setIsSaving] = useState(false);
  const [saveMessage, setSaveMessage] = useState<string>('');

  useEffect(() => {
    if (!isTauriApp()) return;

    const loadCurrentConfiguration = async () => {
      setIsDetectingDatabase(true);
      try {
        const registry = await tauriConfig.getCurrentConfiguration();
        if (registry && registry.databases.length > 0) {
          // Pre-fill from the first configured DB so an edit-in-place feels
          // sensible (full multi-DB editing comes in F3).
          const first = registry.databases[0];
          setDatabaseName(first.name);
          setDatabasePath(first.db_path);
          setImageDirs(first.image_dirs);
        } else {
          const defaultPath = await tauriFileSystem.getDefaultNinaPath();
          if (defaultPath) {
            setDatabasePath(defaultPath);
          }
        }
      } catch (error) {
        console.error('Failed to load configuration:', error);
        try {
          const defaultPath = await tauriFileSystem.getDefaultNinaPath();
          if (defaultPath) {
            setDatabasePath(defaultPath);
          }
        } catch (err) {
          console.error('Failed to detect default database path:', err);
        }
      } finally {
        setIsDetectingDatabase(false);
      }
    };

    if (isOpen) {
      loadCurrentConfiguration();
      setSaveMessage('');
    }
  }, [isOpen]);

  const handlePickDatabase = async () => {
    try {
      const path = await tauriFileSystem.pickDatabaseFile();
      if (path) {
        setDatabasePath(path);
      }
    } catch (error) {
      console.error('Failed to pick database file:', error);
    }
  };

  const handlePickImageDirectory = async () => {
    try {
      const path = await tauriFileSystem.pickImageDirectory();
      if (path) {
        setImageDirs([...imageDirs, path]);
      }
    } catch (error) {
      console.error('Failed to pick image directory:', error);
    }
  };

  const handleRemoveImageDir = (index: number) => {
    setImageDirs(imageDirs.filter((_, i) => i !== index));
  };

  const handleSave = async () => {
    if (!databasePath.trim()) {
      setSaveMessage('Please select a database file');
      return;
    }

    setIsSaving(true);
    setSaveMessage('');

    try {
      // Look up the current registry. If a DB with this canonical path is
      // already present, update it (replace name/image_dirs). Otherwise add.
      const registry = await tauriConfig.getCurrentConfiguration();
      const existing = registry?.databases.find(
        (d) => d.db_path === databasePath.trim()
      );

      const inferredName =
        databaseName.trim() ||
        databasePath.split(/[\\/]/).pop()?.replace(/\.[^.]+$/, '') ||
        'Database';

      let success = false;
      if (existing && registry) {
        // Update in place. Slug stays the same.
        const updated: typeof registry = {
          ...registry,
          databases: registry.databases.map((d) =>
            d === existing
              ? { ...d, name: inferredName, image_dirs: imageDirs }
              : d
          ),
        };
        success = await tauriConfig.saveConfiguration(updated);
      } else {
        const added = await tauriConfig.addDatabase(
          inferredName,
          databasePath.trim(),
          imageDirs
        );
        success = !!added;
      }

      if (success) {
        setSaveMessage('Configuration saved! Restarting server...');

        try {
          const serverRestartSuccess = await tauriConfig.restartServer();
          if (serverRestartSuccess) {
            setSaveMessage('Configuration applied successfully! Reloading interface...');
            setTimeout(() => {
              window.location.reload();
            }, 2000);
          } else {
            setSaveMessage('Applying configuration... (restarting application)');
            setTimeout(async () => {
              await tauriConfig.restartApplication();
            }, 1000);
          }
        } catch (restartError) {
          console.error('Failed to restart server:', restartError);
          setSaveMessage('Configuration saved but failed to restart - please restart manually');
        }
      } else {
        setSaveMessage('Failed to save configuration');
      }
    } catch (error) {
      console.error('Error saving configuration:', error);
      setSaveMessage('Error saving configuration');
    } finally {
      setIsSaving(false);
    }
  };


  if (!isOpen) {
    return null;
  }

  return (
    <div className="tauri-settings modal-overlay" onClick={onClose}>
      <div className="modal-content" onClick={e => e.stopPropagation()}>
        <div className="modal-header">
          <h2>PSF Guard Settings</h2>
          <button className="close-button" onClick={onClose}>×</button>
        </div>
        
        <div className="modal-body">
          <div className="welcome-message">
            <h3>🚀 Welcome to PSF Guard Desktop!</h3>
            <p>To get started, please configure your database and image directories:</p>
            <ul>
              <li><strong>Database</strong>: Your N.I.N.A. scheduler database file</li>
              <li><strong>Image Directories</strong>: Folders containing your FITS image files</li>
            </ul>
          </div>

          <div className="settings-section">
            <h3>Database Configuration</h3>

            {isDetectingDatabase && (
              <div className="detecting-database">
                🔍 Detecting N.I.N.A. database...
              </div>
            )}

            <div className="database-config">
              <label>Display name (optional):</label>
              <input
                type="text"
                value={databaseName}
                onChange={(e) => setDatabaseName(e.target.value)}
                placeholder="e.g. Imaging Rig (defaults to filename)"
                className="file-path-input"
              />
            </div>

            <div className="database-config">
              <label>N.I.N.A. Database File:</label>
              <div className="file-input-group">
                <input
                  type="text"
                  value={databasePath}
                  onChange={(e) => setDatabasePath(e.target.value)}
                  placeholder="Select or enter database path"
                  className="file-path-input"
                />
                <button onClick={handlePickDatabase} className="browse-button">
                  Browse...
                </button>
              </div>
              {databasePath ? (
                <div className="path-info">
                  ✅ {databasePath}
                </div>
              ) : (
                <div className="path-info" style={{color: '#666'}}>
                  💡 Click "Browse..." to select your N.I.N.A. database file (usually ends in .sqlite)
                </div>
              )}
            </div>
          </div>

          <div className="settings-section">
            <h3>Image Directories</h3>
            <p>Add directories containing your FITS image files:</p>
            
            <button onClick={handlePickImageDirectory} className="add-directory-button">
              + Add Image Directory
            </button>
            
            {imageDirs.length > 0 && (
              <div className="image-directories">
                {imageDirs.map((dir, index) => (
                  <div key={index} className="image-directory-item">
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
            
            {imageDirs.length === 0 && (
              <div className="no-directories">
                No image directories configured. You can add them later or use the file picker in the UI.
              </div>
            )}
          </div>
        </div>
        
        <div className="modal-footer">
          {saveMessage && (
            <div className={`save-message ${saveMessage.includes('Error') || saveMessage.includes('Failed') ? 'error' : 'success'}`}>
              {saveMessage}
            </div>
          )}
          
          <div className="modal-buttons">
            <button onClick={onClose} className="cancel-button" disabled={isSaving}>
              Cancel
            </button>
            <button 
              onClick={handleSave} 
              className="save-button"
              disabled={!databasePath.trim() || isSaving}
            >
              {isSaving ? 'Applying Configuration...' : 'Save & Apply Settings'}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}