import { useState, useEffect } from 'react';
import { Outlet, useNavigate, useLocation } from 'react-router-dom';
import { useHotkeys } from 'react-hotkeys-hook';
import ProjectTargetSelector from './components/ProjectTargetSelector';
import KeyboardShortcutHelp from './components/KeyboardShortcutHelp';
import ServerInfoPanel from './components/ServerInfoPanel';
import CacheRefreshStatus from './components/CacheRefreshStatus';
import AggregatedCacheStatus from './components/AggregatedCacheStatus';
import TauriSettings from './components/TauriSettings';
import { useGridState } from './hooks/useUrlState';
import { isTauriApp, tauriConfig } from './utils/tauri';
import { apiClient } from './api/client';
import './App.css';

function App() {
  const navigate = useNavigate();
  const location = useLocation();
  const { showStats, setShowStats } = useGridState();
  const [showHelp, setShowHelp] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  // We track this only to short-circuit checks against Tauri-only commands;
  // the modal itself is shown regardless of mode.
  const [, setIsTauri] = useState(false);

  // Check configuration on mount. In both Tauri and browser/CLI-server mode,
  // we pop the settings modal automatically when no databases are configured.
  useEffect(() => {
    let cancelled = false;

    const checkConfiguration = async () => {
      const tauriDetected = isTauriApp();
      if (!cancelled) setIsTauri(tauriDetected);

      try {
        // Prefer the Tauri validation when available (it can detect a config
        // file present but pointing at a missing DB). In browser mode fall
        // back to the HTTP listing.
        let hasValid = false;
        if (tauriDetected) {
          hasValid = await tauriConfig.isConfigurationValid();
        } else {
          const dbs = await apiClient.getDatabases();
          hasValid = dbs.length > 0;
        }
        if (!cancelled && !hasValid) {
          console.log('No databases configured — opening settings modal');
          setShowSettings(true);
        }
      } catch (error) {
        console.error('Failed to check configuration:', error);
        if (!cancelled) setShowSettings(true);
      }
    };

    checkConfiguration();
    // Re-check after a delay in case Tauri globals load late.
    const handle = setTimeout(checkConfiguration, 1000);

    // Let any component request opening settings via a window event (e.g.
    // the Overview empty-state button).
    const openHandler = () => setShowSettings(true);
    window.addEventListener('psf-guard:open-settings', openHandler);

    return () => {
      cancelled = true;
      clearTimeout(handle);
      window.removeEventListener('psf-guard:open-settings', openHandler);
    };
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
          {/* CacheRefreshStatus renders for scoped views (?db= in URL);
              AggregatedCacheStatus renders for unscoped views (e.g. overview).
              Each is internally a no-op in the other case, so both can be
              mounted unconditionally. */}
          <CacheRefreshStatus />
          <AggregatedCacheStatus />
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
