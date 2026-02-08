import { useState, useEffect } from 'react';
import { Outlet, useNavigate, useLocation } from 'react-router-dom';
import { useHotkeys } from 'react-hotkeys-hook';
import ProjectTargetSelector from './components/ProjectTargetSelector';
import KeyboardShortcutHelp from './components/KeyboardShortcutHelp';
import ServerInfoPanel from './components/ServerInfoPanel';
import CacheRefreshStatus from './components/CacheRefreshStatus';
import TauriSettings from './components/TauriSettings';
import { useGridState } from './hooks/useUrlState';
import { isTauriApp, tauriConfig } from './utils/tauri';
import './App.css';

function App() {
  const navigate = useNavigate();
  const location = useLocation();
  const { showStats, setShowStats } = useGridState();
  const [showHelp, setShowHelp] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [isTauri, setIsTauri] = useState(false);

  // Check if we're in Tauri mode and handle settings
  useEffect(() => {
    const checkTauriAndConfiguration = async () => {
      const tauriDetected = isTauriApp();
      console.log('App mounted, checking if Tauri app:', tauriDetected);
      setIsTauri(tauriDetected);
      
      if (tauriDetected) {
        try {
          // Use the backend validation to check if configuration is complete and valid
          const isValid = await tauriConfig.isConfigurationValid();
          
          if (!isValid) {
            console.log('Tauri detected with invalid/incomplete configuration, showing settings modal');
            setShowSettings(true);
          } else {
            console.log('Tauri detected with valid configuration, not showing settings modal');
          }
        } catch (error) {
          console.error('Failed to check configuration validity, showing settings modal:', error);
          setShowSettings(true);
        }
      } else {
        console.log('Not a Tauri app, skipping auto-settings');
      }
    };

    // Check immediately
    checkTauriAndConfiguration();
    
    // Also check after a delay in case Tauri globals load later
    setTimeout(checkTauriAndConfiguration, 1000);
  }, []);

  // Keyboard shortcut for help
  useHotkeys('?', () => setShowHelp(true), []);
  
  const isOnOverview = location.pathname === '/' || location.pathname === '/overview';
  const isOnGrid = location.pathname === '/grid';
  const isOnSequence = location.pathname === '/sequence';

  return (
    <div className="app">
      <header className="app-header compact">
        <div className="header-left">
          <h1 
            onClick={() => navigate('/')}
            style={{ cursor: 'pointer' }}
            title="Go to Overview"
          >
            PSF Guard
          </h1>
        </div>
        
        <div className="header-center">
          <CacheRefreshStatus />
          {!isOnOverview && <ProjectTargetSelector />}
        </div>
        
        <div className="header-right">
          {!isOnOverview && (
            <button onClick={() => navigate('/')} className="header-button">
              Overview
            </button>
          )}
          {!isOnGrid && (
            <button onClick={() => navigate('/grid')} className="header-button">
              Images
            </button>
          )}
          {!isOnSequence && (
            <button onClick={() => navigate('/sequence')} className="header-button">
              Sequence
            </button>
          )}
          {!isOnOverview && (
            <button onClick={() => setShowStats(!showStats)} className="header-button">
              {showStats ? 'Hide Stats' : 'Stats'}
            </button>
          )}
          {isTauri && (
            <button onClick={() => setShowSettings(true)} className="header-button">
              Settings
            </button>
          )}
          <button onClick={() => setShowHelp(true)} className="header-button">
            Help
          </button>
          <ServerInfoPanel />
        </div>
      </header>

      <main className="app-main">
        <Outlet />
      </main>

      {showHelp && (
        <KeyboardShortcutHelp onClose={() => setShowHelp(false)} />
      )}
      
      {showSettings && isTauri && (
        <TauriSettings 
          isOpen={showSettings} 
          onClose={() => setShowSettings(false)} 
        />
      )}
    </div>
  );
}

export default App;
