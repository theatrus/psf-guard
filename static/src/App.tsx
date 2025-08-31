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
      <header className="app-header">
        <h1>PSF Guard - Image Grading</h1>
        <div className="header-actions">
          <button onClick={() => setShowStats(!showStats)} className="help-button">
            {showStats ? 'Hide Stats' : 'Show Stats'}
          </button>
          <button onClick={() => setShowHelp(true)} className="help-button">
            Help (?)
          </button>
          <ServerInfoPanel />
        </div>
      </header>

      <div className="app-controls">
        <ProjectTargetSelector />
      </div>

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
