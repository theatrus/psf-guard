import { useState, useEffect } from 'react';
import { isTauriApp, tauriFileSystem } from '../utils/tauri';
import './TauriSettings.css';

interface TauriSettingsProps {
  isOpen: boolean;
  onClose: () => void;
}

export default function TauriSettings({ isOpen, onClose }: TauriSettingsProps) {
  const [databasePath, setDatabasePath] = useState<string>('');
  const [imageDirs, setImageDirs] = useState<string[]>([]);
  const [isDetectingDatabase, setIsDetectingDatabase] = useState(false);

  useEffect(() => {
    // Only show in Tauri mode
    if (!isTauriApp()) return;

    // Try to get default N.I.N.A. database path
    const detectDefaultDatabase = async () => {
      setIsDetectingDatabase(true);
      try {
        const defaultPath = await tauriFileSystem.getDefaultNinaPath();
        if (defaultPath) {
          setDatabasePath(defaultPath);
        }
      } catch (error) {
        console.error('Failed to detect default database path:', error);
      } finally {
        setIsDetectingDatabase(false);
      }
    };

    if (isOpen) {
      detectDefaultDatabase();
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

  const handleSave = () => {
    // For now, just close the modal
    // In a full implementation, this would restart the server with new paths
    onClose();
  };

  if (!isTauriApp() || !isOpen) {
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
              {databasePath && (
                <div className="path-info">
                  📁 {databasePath}
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
          <button onClick={onClose} className="cancel-button">
            Cancel
          </button>
          <button 
            onClick={handleSave} 
            className="save-button"
            disabled={!databasePath.trim()}
          >
            Save Settings
          </button>
        </div>
      </div>
    </div>
  );
}