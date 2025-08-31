import { useState } from 'react';
import { Outlet } from 'react-router-dom';
import { useHotkeys } from 'react-hotkeys-hook';
import ProjectTargetSelector from './components/ProjectTargetSelector';
import KeyboardShortcutHelp from './components/KeyboardShortcutHelp';
import ServerInfoPanel from './components/ServerInfoPanel';
import { useGridState } from './hooks/useUrlState';
import './App.css';

function App() {
  const { showStats, setShowStats } = useGridState();
  const [showHelp, setShowHelp] = useState(false);

  // Keyboard shortcut for help
  useHotkeys('?', () => setShowHelp(true), []);

  return (
    <div className="app">
      <header className="app-header compact">
        <div className="header-left">
          <h1>PSF Guard</h1>
        </div>
        
        <div className="header-center">
          <ProjectTargetSelector />
        </div>
        
        <div className="header-right">
          <button onClick={() => setShowStats(!showStats)} className="header-button">
            {showStats ? 'Hide Stats' : 'Stats'}
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
    </div>
  );
}

export default App;
