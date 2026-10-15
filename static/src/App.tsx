import { useState, useEffect, useRef } from 'react';
import { Outlet, useNavigate, useLocation } from 'react-router-dom';
import { useHotkeys } from 'react-hotkeys-hook';
import { useQuery } from '@tanstack/react-query';
import ProjectTargetSelector from './components/ProjectTargetSelector';
import RigTargetSelect from './components/header/RigTargetSelect';
import { useCurrentPlan } from './components/header/useCurrentPlan';
import ActivityChip from './components/header/ActivityChip';
import LiveChip from './components/header/LiveChip';
import HeaderMenu from './components/header/HeaderMenu';
import ViewTab from './components/header/ViewTab';
import { ChartColumn, Crosshair, Images, LibraryBig, SquareStack, Telescope, TestTubeDiagonal } from 'lucide-react';
import { useHeaderFit } from './components/header/useHeaderFit';
import KeyboardShortcutHelp from './components/KeyboardShortcutHelp';
import ServerInfoPanel from './components/ServerInfoPanel';
import SiteBanner from './components/SiteBanner';
import UpdateNotice from './components/UpdateNotice';
import BuildRefresh from './components/BuildRefresh';
import TauriSettings from './components/TauriSettings';
import { DesktopServerGate } from './components/DesktopServerSettings';
import { isOverviewPath, isPlanningPath, isSkyPath, useDbProjectTarget } from './hooks/useUrlState';
import { scopeForView } from './utils/viewScope';
import { isTauriApp, tauriConfig } from './utils/tauri';
import {
  OPEN_SETTINGS_EVENT,
  settingsFieldOf,
  settingsIntentOf,
  settingsRigOf,
  type SettingsIntent,
  type SettingsRig,
} from './utils/settingsIntent';
import { apiClient } from './api/client';
import AuthGate from './auth/AccessContext';
import { useAccess } from './auth/access';
import { useAccountPreferences } from './hooks/useAccountPreferences';
import './App.css';

function AppContent() {
  const navigate = useNavigate();
  const location = useLocation();
  const isOnPlan = isPlanningPath(location.pathname);
  // The review scope: a database, and usually a project, parked in the URL.
  const { dbId } = useDbProjectTarget();
  const hasReviewScope = dbId !== null;
  const plan = useCurrentPlan();
  const isOnWorkspace = location.pathname === '/plan' && !!plan.current;
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
  // Planning's own params (which workspace is open) stay behind, and so
  // does a view's own thumbnail size when the view changes.
  const toScoped = (path: string) => {
    const query = scopeForView(location.search, location.pathname, path).toString();
    return query ? `${path}?${query}` : path;
  };
  const [showHelp, setShowHelp] = useState(false);
  const [showServerInfo, setShowServerInfo] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  // Which form the settings modal should land on. Set by whoever asked for
  // the modal; read once when it mounts.
  const [settingsIntent, setSettingsIntent] = useState<SettingsIntent | null>(
    null
  );
  // With the Calibration tab: the rig whose own settings to open.
  const [settingsRig, setSettingsRig] = useState<SettingsRig | null>(null);
  // With the Calibration tab: the setting to bring into view.
  const [settingsField, setSettingsField] = useState<string | null>(null);
  // We track this only to short-circuit checks against Tauri-only commands;
  // the modal itself is shown regardless of mode.
  const [, setIsTauri] = useState(false);
  const access = useAccess();
  // A signed-in user's View choices follow them from browser to browser.
  useAccountPreferences(access.status.username);
  // Settings opens by itself at most once: the check repeats for late Tauri
  // globals, and a user who closed it should not see it come back.
  const autoOpened = useRef(false);
  const headerRef = useRef<HTMLElement>(null);
  const headerStacked = useHeaderFit(headerRef);

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
        if (!cancelled && !isOnPlan && access.canWrite && !hasValid && managementAllowed && !autoOpened.current) {
          console.log('No databases configured — opening settings modal');
          autoOpened.current = true;
          setShowSettings(true);
        }
      } catch (error) {
        console.error('Failed to check configuration:', error);
        if (!cancelled && !isOnPlan && access.canWrite && !autoOpened.current) {
          autoOpened.current = true;
          setShowSettings(true);
        }
      }
    };

    checkConfiguration();
    // Re-check after a delay in case Tauri globals load late.
    const handle = setTimeout(checkConfiguration, 1000);

    // Let any component request opening settings via a window event (e.g.
    // the Overview empty-state button).
    const openHandler = (event: Event) => {
      if (!access.canWrite) return;
      // Clear first so asking twice for the same form still lands on it.
      setSettingsIntent(null);
      setSettingsRig(settingsRigOf(event));
      setSettingsField(settingsFieldOf(event));
      queueMicrotask(() => setSettingsIntent(settingsIntentOf(event)));
      setShowSettings(true);
    };
    window.addEventListener(OPEN_SETTINGS_EVENT, openHandler);

    return () => {
      cancelled = true;
      clearTimeout(handle);
      window.removeEventListener(OPEN_SETTINGS_EVENT, openHandler);
    };
  }, [access.canWrite, isOnPlan]);

  // Keyboard shortcut for help
  useHotkeys('?', () => setShowHelp(true), []);
  
  const isOnOverview = isOverviewPath(location.pathname);
  const isOnGrid = location.pathname === '/grid';
  const isOnSequence = location.pathname === '/sequence';
  const isOnStacks = location.pathname === '/stacks';
  const isOnCalibration = location.pathname === '/calibration';
  const isOnSky = isSkyPath(location.pathname);

  return (
    <div className="app">
      <header ref={headerRef} className={`app-header compact${headerStacked ? ' is-stacked' : ''}`}>
        <div className="header-brand" data-header-part="brand">
          <button
            type="button"
            className="brand-button"
            onClick={() => navigate(toScoped('/'))}
            title="Go to Library"
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

        <nav className="header-view-tabs header-nav" aria-label="Views" data-header-part="nav">
          {/* The Sky follows no scope, so it stands apart, ahead of the rest. */}
          <ViewTab
            label="Sky"
            icon={Telescope}
            current={isOnSky}
            onClick={() => navigate(toScoped('/sky'))}
          />
          <ViewTab
            label="Library"
            icon={LibraryBig}
            current={isOnOverview}
            onClick={() => navigate(toScoped('/'))}
          />
          {/* The scope, from the whole to the part: the project (a plan shot
              by several rigs is one), its workspace, then which rig and
              target the views after it show. */}
          <div className="header-group header-scope" role="group" aria-label="Scope">
            <ProjectTargetSelector />
            {plan.enabled && (
              <ViewTab
                label="Planning"
                icon={Crosshair}
                current={isOnWorkspace}
                disabled={!plan.current}
                title={plan.current ? undefined : 'Choose a project with a plan first'}
                onClick={() => plan.current && navigate(plan.hrefFor(plan.current))}
              />
            )}
            <span className="header-scope-divider" aria-hidden="true" />
            <RigTargetSelect />
            <ViewTab
              label="Images"
              icon={Images}
              current={isOnGrid}
              disabled={!hasReviewScope}
              title={hasReviewScope ? undefined : 'Choose a project or database first'}
              onClick={() => navigate(toScoped('/grid'))}
            />
            <ViewTab
              label="Sequence"
              icon={ChartColumn}
              current={isOnSequence}
              disabled={!hasReviewScope}
              title={hasReviewScope ? undefined : 'Choose a project or database first'}
              onClick={() => navigate(toScoped('/sequence'))}
            />
            {/* In pipeline order: the lights, their sequence, the calibration
                that serves them, then the stacks they make. Calibration
                follows the database and rig in scope; with no database it
                opens on the first. */}
            <ViewTab
              label="Calibration"
              icon={TestTubeDiagonal}
              current={isOnCalibration}
              onClick={() => navigate(toScoped('/calibration'))}
            />
            <ViewTab
              label="Stacks"
              icon={SquareStack}
              current={isOnStacks}
              disabled={!hasReviewScope}
              title={hasReviewScope ? undefined : 'Choose a project or database first'}
              onClick={() => navigate(toScoped('/stacks'))}
            />
          </div>
        </nav>

        <div className="header-utilities" data-header-part="utilities">
          {/* Background work on every database, kept to one small chip;
              hover or click opens the queue. */}
          <ActivityChip />
          <LiveChip />
          <HeaderMenu
            onSettings={access.canWrite ? () => setShowSettings(true) : undefined}
            onShortcuts={() => setShowHelp(true)}
            onServerInfo={() => setShowServerInfo(true)}
            signedInAs={access.status.authentication_required ? access.status.username ?? 'user' : null}
            onSignOut={access.status.authentication_required ? () => void access.logout() : undefined}
          />
        </div>
      </header>

      <SiteBanner banner={serverInfo?.banner} />
      <UpdateNotice installedVersion={serverInfo?.version} />
      <BuildRefresh />

      <main className="app-main">
        <Outlet />
      </main>

      {showHelp && (
        <KeyboardShortcutHelp onClose={() => setShowHelp(false)} />
      )}

      {showServerInfo && <ServerInfoPanel onClose={() => setShowServerInfo(false)} />}
      
      {showSettings && access.canWrite && (
        <TauriSettings
          isOpen={showSettings}
          initialIntent={settingsIntent}
          initialRig={settingsRig}
          initialField={settingsField}
          onClose={() => {
            setShowSettings(false);
            setSettingsIntent(null);
            setSettingsRig(null);
            setSettingsField(null);
          }}
        />
      )}
    </div>
  );
}

export default function App() {
  return (
    <DesktopServerGate>
      <AuthGate>
        <AppContent />
      </AuthGate>
    </DesktopServerGate>
  );
}
