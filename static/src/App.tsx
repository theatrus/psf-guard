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

  // Carry the active (db, project, target, filter…) query context when switching
  // between scoped views, so navigation never drops the ?db= slug and strands
  // the user on an empty view.
  const toScoped = (path: string) =>
    location.search ? `${path}${location.search}` : path;
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
        let managementAllowed = tauriDetected;
        if (tauriDetected) {
          hasValid = await tauriConfig.isConfigurationValid();
        } else {
          const [dbs, info] = await Promise.all([
            apiClient.getDatabases(),
            apiClient.getServerInfo(),
          ]);
          hasValid = dbs.length > 0;
          managementAllowed = info.allow_database_management;
        }
        // Only auto-pop the modal when we can actually do something about it.
        // If management is disabled and there are no DBs, leave the user on
        // the overview's empty state where they can read the explanation
        // without a modal blocking them.
        if (!cancelled && !hasValid && managementAllowed) {
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
      <header className={`app-header compact${isOnOverview ? ' app-header--overview' : ''}`}>
        <div className="header-brand">
          <button
            type="button"
            className="brand-button"
            onClick={() => navigate('/')}
            title="Go to Overview"
          >
            PSF Guard
          </button>
        </div>

        <div className={`header-context${isOnOverview ? ' header-context--overview' : ''}`}>
          {!isOnOverview && <ProjectTargetSelector />}
          <div className="header-cache-slot" aria-live="polite">
          {/* CacheRefreshStatus renders for scoped views (?db= in URL);
              AggregatedCacheStatus renders for unscoped views (e.g. overview).
              This fixed slot keeps status changes from moving the header. */}
            <CacheRefreshStatus className="header-cache-progress" />
            <AggregatedCacheStatus className="header-cache-progress" />
          </div>
        </div>

        <nav className="header-view-tabs" aria-label="Views">
          <button
            type="button"
            onClick={() => navigate('/')}
            className="header-button"
            aria-current={isOnOverview ? 'page' : undefined}
          >
            Overview
          </button>
          <button
            type="button"
            onClick={() => navigate(toScoped('/grid'))}
            className="header-button"
            aria-current={isOnGrid ? 'page' : undefined}
          >
            Images
          </button>
          <button
            type="button"
            onClick={() => navigate(toScoped('/sequence'))}
            className="header-button"
            aria-current={isOnSequence ? 'page' : undefined}
          >
            Sequence
          </button>
        </nav>

        <div className="header-utilities">
          {isOnGrid && (
            <button
              type="button"
              onClick={() => setShowStats(!showStats)}
              className="header-button utility-button"
              aria-pressed={showStats}
              title={showStats ? 'Hide image statistics' : 'Show image statistics'}
            >
              <span className="utility-icon" aria-hidden="true">▥</span>
              <span className="utility-label">
              {showStats ? 'Hide Stats' : 'Stats'}
              </span>
            </button>
          )}
          <button
            type="button"
            onClick={() => setShowSettings(true)}
            className="header-button utility-button"
            title="Settings"
          >
            <span className="utility-icon" aria-hidden="true">⚙</span>
            <span className="utility-label">Settings</span>
          </button>
          <button
            type="button"
            onClick={() => setShowHelp(true)}
            className="header-button utility-button"
            title="Keyboard shortcuts"
          >
            <span className="utility-icon" aria-hidden="true">?</span>
            <span className="utility-label">Help</span>
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
