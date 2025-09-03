import { useState, useEffect } from 'react';
import { isTauriApp, tauriFileSystem, tauriConfig } from '../utils/tauri';
import type { TauriConfig } from '../utils/tauri';
import './TauriSettings.css';

interface TauriSettingsProps {
  isOpen: boolean;
  onClose: () => void;
}

export default function TauriSettings({ isOpen, onClose }: TauriSettingsProps) {
  const [databasePath, setDatabasePath] = useState<string>('');
  const [imageDirs, setImageDirs] = useState<string[]>([]);
  const [isDetectingDatabase, setIsDetectingDatabase] = useState(false);
  const [isSaving, setIsSaving] = useState(false);
  const [saveMessage, setSaveMessage] = useState<string>('');
  const [showRestartPrompt, setShowRestartPrompt] = useState(false);

  useEffect(() => {
    // Only show in Tauri mode
    if (!isTauriApp()) return;

    const loadCurrentConfiguration = async () => {
      setIsDetectingDatabase(true);
      try {
        // Load existing configuration
        const currentConfig = await tauriConfig.getCurrentConfiguration();
        if (currentConfig) {
          setDatabasePath(currentConfig.database_path || '');
          setImageDirs(currentConfig.image_directories || []);
        } else {
          // Fall back to detecting default database path
          const defaultPath = await tauriFileSystem.getDefaultNinaPath();
          if (defaultPath) {
            setDatabasePath(defaultPath);
          }
        }
      } catch (error) {
        console.error('Failed to load configuration:', error);
        // Try to get default N.I.N.A. database path as fallback
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
      setSaveMessage(''); // Clear any previous messages
      setShowRestartPrompt(false); // Clear restart prompt
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
      const config: TauriConfig = {
        database_path: databasePath.trim(),
        image_directories: imageDirs,
      };

      const success = await tauriConfig.saveConfiguration(config);
      if (success) {
        setSaveMessage('Configuration saved successfully!');
        setShowRestartPrompt(true);
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

  const handleRestart = async () => {
    try {
      await tauriConfig.restartApplication();
      // Application will restart, so this code may not execute
    } catch (error) {
      console.error('Failed to restart application:', error);
      setSaveMessage('Failed to restart application');
    }
  };

  if (!isOpen) {
    return null;
  }

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal-content tauri-settings" onClick={e => e.stopPropagation()}>
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
          
          {showRestartPrompt ? (
            <div className="restart-prompt">
              <p>⚠️ Configuration saved! Restart the application to apply changes.</p>
              <div className="modal-buttons">
                <button onClick={onClose} className="cancel-button">
                  Continue Without Restart
                </button>
                <button onClick={handleRestart} className="restart-button">
                  Restart Now
                </button>
              </div>
            </div>
          ) : (
            <div className="modal-buttons">
              <button onClick={onClose} className="cancel-button" disabled={isSaving}>
                Cancel
              </button>
              <button 
                onClick={handleSave} 
                className="save-button"
                disabled={!databasePath.trim() || isSaving}
              >
                {isSaving ? 'Saving...' : 'Save Settings'}
              </button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}