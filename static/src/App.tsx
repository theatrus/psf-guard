import { useState, useEffect } from 'react';
import { Outlet, useNavigate, useLocation } from 'react-router-dom';
import { useHotkeys } from 'react-hotkeys-hook';
import { useQuery } from '@tanstack/react-query';
import ProjectTargetSelector from './components/ProjectTargetSelector';
import KeyboardShortcutHelp from './components/KeyboardShortcutHelp';
import ServerInfoPanel from './components/ServerInfoPanel';
import SiteBanner from './components/SiteBanner';
import UpdateNotice from './components/UpdateNotice';
import DatabaseActivityStatus from './components/DatabaseActivityStatus';
import AggregatedCacheStatus from './components/AggregatedCacheStatus';
import TauriSettings from './components/TauriSettings';
import { isOverviewPath, useGridState } from './hooks/useUrlState';
import { isTauriApp, tauriConfig } from './utils/tauri';
import {
  OPEN_SETTINGS_EVENT,
  settingsIntentOf,
  type SettingsIntent,
} from './utils/settingsIntent';
import { apiClient } from './api/client';
import AuthGate from './auth/AccessContext';
import { useAccess } from './auth/access';
import './App.css';

function AppContent() {
  const navigate = useNavigate();
  const location = useLocation();
  const { showStats, setShowStats } = useGridState();
  const { data: serverInfo } = useQuery({
    queryKey: ['serverInfo'],
    queryFn: apiClient.getServerInfo,
    staleTime: 5 * 60 * 1000,
  });

  // Carry the active (db, project, target, filter…) query context when switching
  // between views, so navigation never drops the ?db= slug and strands the user
  // on an empty view. The Overview keeps it too: it shows every database, but
  // holding the scope lets it point at the project the user left and hand the
  // same scope back to Images or Sequence.
  const toScoped = (path: string) =>
    location.search ? `${path}${location.search}` : path;
  const [showHelp, setShowHelp] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  // Which form the settings modal should land on. Set by whoever asked for
  // the modal; read once when it mounts.
  const [settingsIntent, setSettingsIntent] = useState<SettingsIntent | null>(
    null
  );
  // We track this only to short-circuit checks against Tauri-only commands;
  // the modal itself is shown regardless of mode.
  const [, setIsTauri] = useState(false);
  const access = useAccess();

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
        if (!cancelled && access.canWrite && !hasValid && managementAllowed) {
          console.log('No databases configured — opening settings modal');
          setShowSettings(true);
        }
      } catch (error) {
        console.error('Failed to check configuration:', error);
        if (!cancelled && access.canWrite) setShowSettings(true);
      }
    };

    checkConfiguration();
    // Re-check after a delay in case Tauri globals load late.
    const handle = setTimeout(checkConfiguration, 1000);

    // Let any component request opening settings via a window event (e.g.
    // the Overview empty-state button).
    const openHandler = (event: Event) => {
      if (!access.canWrite) return;
      setSettingsIntent(settingsIntentOf(event));
      setShowSettings(true);
    };
    window.addEventListener(OPEN_SETTINGS_EVENT, openHandler);

    return () => {
      cancelled = true;
      clearTimeout(handle);
      window.removeEventListener(OPEN_SETTINGS_EVENT, openHandler);
    };
  }, [access.canWrite]);

  // Keyboard shortcut for help
  useHotkeys('?', () => setShowHelp(true), []);
  
  const isOnOverview = isOverviewPath(location.pathname);
  const isOnGrid = location.pathname === '/grid';
  const isOnSequence = location.pathname === '/sequence';

  return (
    <div className="app">
      <header className={`app-header compact${isOnOverview ? ' app-header--overview' : ''}`}>
        <div className="header-brand">
          <button
            type="button"
            className="brand-button"
            onClick={() => navigate(toScoped('/'))}
            title="Go to Overview"
          >
            <img
              className="brand-logo"
              src="/psf-guard.svg"
              alt=""
              aria-hidden="true"
            />
            <span>PSF Guard</span>
          </button>
        </div>

        <div className={`header-context${isOnOverview ? ' header-context--overview' : ''}`}>
          {!isOnOverview && <ProjectTargetSelector />}
          <div className="header-cache-slot" aria-live="polite">
          {/* Scoped views show the active database's refresh or quality job;
              unscoped views merge active jobs across databases. This fixed
              slot keeps status changes from moving the header. */}
            <DatabaseActivityStatus className="header-cache-progress" />
            <AggregatedCacheStatus className="header-cache-progress" />
          </div>
        </div>

        <nav className="header-view-tabs" aria-label="Views">
          <button
            type="button"
            onClick={() => navigate(toScoped('/'))}
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
          {access.canWrite && (
            <button
              type="button"
              onClick={() => setShowSettings(true)}
              className="header-button utility-button"
              title="Settings"
            >
              <span className="utility-icon" aria-hidden="true">⚙</span>
              <span className="utility-label">Settings</span>
            </button>
          )}
          {access.status.authentication_required && (
            <>
              <span
                className={`access-badge ${access.canWrite ? 'read-write' : 'read-only'}`}
                title={`Signed in as ${access.status.username ?? 'user'}`}
              >
                {access.canWrite ? 'Editor' : 'Read only'}
              </span>
              <button
                type="button"
                onClick={() => void access.logout()}
                className="header-button utility-button"
                title="Sign out"
              >
                <span className="utility-label">Sign out</span>
              </button>
            </>
          )}
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

      <SiteBanner banner={serverInfo?.banner} />
      <UpdateNotice installedVersion={serverInfo?.version} />

      <main className="app-main">
        <Outlet />
      </main>

      {showHelp && (
        <KeyboardShortcutHelp onClose={() => setShowHelp(false)} />
      )}
      
      {showSettings && access.canWrite && (
        <TauriSettings
          isOpen={showSettings}
          initialIntent={settingsIntent}
          onClose={() => {
            setShowSettings(false);
            setSettingsIntent(null);
          }}
        />
      )}
    </div>
  );
}

export default function App() {
  return (
    <AuthGate>
      <AppContent />
    </AuthGate>
  );
}
