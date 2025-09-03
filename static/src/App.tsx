import { useState, useEffect } from 'react';
import { Outlet, useNavigate, useLocation } from 'react-router-dom';
import { useHotkeys } from 'react-hotkeys-hook';
import ProjectTargetSelector from './components/ProjectTargetSelector';
import KeyboardShortcutHelp from './components/KeyboardShortcutHelp';
import ServerInfoPanel from './components/ServerInfoPanel';
import CacheRefreshStatus from './components/CacheRefreshStatus';
import TauriSettings from './components/TauriSettings';
import { useGridState } from './hooks/useUrlState';
import { isTauriApp } from './utils/tauri';
import './App.css';

function App() {
  const navigate = useNavigate();
  const location = useLocation();
  const { showStats, setShowStats } = useGridState();
  const [showHelp, setShowHelp] = useState(false);
  const [showSettings, setShowSettings] = useState(false);

  // Auto-show settings in Tauri mode on first start
  useEffect(() => {
    const checkTauri = () => {
      console.log('App mounted, checking if Tauri app:', isTauriApp());
      console.log('Window.__TAURI__ exists:', typeof window !== 'undefined' && '__TAURI__' in window);
      
      if (isTauriApp()) {
        console.log('Tauri detected, showing settings modal');
        // Show settings modal automatically when app starts
        setShowSettings(true);
      } else {
        console.log('Not a Tauri app, skipping auto-settings');
      }
    };

    // Check immediately
    checkTauri();
    
    // Also check after a delay in case Tauri globals load later
    setTimeout(checkTauri, 1000);
  }, []);

  // Keyboard shortcut for help
  useHotkeys('?', () => setShowHelp(true), []);
  
  const isOnOverview = location.pathname === '/' || location.pathname === '/overview';
  const isOnGrid = location.pathname === '/grid';

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
          {!isOnOverview && (
            <button onClick={() => setShowStats(!showStats)} className="header-button">
              {showStats ? 'Hide Stats' : 'Stats'}
            </button>
          )}
          <button onClick={() => setShowSettings(true)} className="header-button">
            Settings
          </button>
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
      
      {showSettings && (
        <TauriSettings 
          isOpen={showSettings} 
          onClose={() => setShowSettings(false)} 
        />
      )}
    </div>
  );
}

export default App;
