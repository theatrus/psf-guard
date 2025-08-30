import { useState } from 'react';
import { useHotkeys } from 'react-hotkeys-hook';
import ProjectTargetSelector from './components/ProjectTargetSelector';
import GroupedImageGrid from './components/GroupedImageGrid';
import KeyboardShortcutHelp from './components/KeyboardShortcutHelp';
import ServerInfoPanel from './components/ServerInfoPanel';
import './App.css';

function App() {
  const [selectedProjectId, setSelectedProjectId] = useState<number | null>(null);
  const [selectedTargetId, setSelectedTargetId] = useState<number | null>(null);
  const [showHelp, setShowHelp] = useState(false);
  const [showStats, setShowStats] = useState(false);

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
        <ProjectTargetSelector
          selectedProjectId={selectedProjectId}
          selectedTargetId={selectedTargetId}
          onProjectChange={setSelectedProjectId}
          onTargetChange={setSelectedTargetId}
        />
      </div>

      <main className="app-main">
        {selectedProjectId && (
          <GroupedImageGrid
            projectId={selectedProjectId}
            targetId={selectedTargetId}
            useLazyImages={true}
            showStats={showStats}
          />
        )}
        {!selectedProjectId && (
          <div className="empty-state">
            Select a project to begin grading images
          </div>
        )}
      </main>

      {showHelp && (
        <KeyboardShortcutHelp onClose={() => setShowHelp(false)} />
      )}
    </div>
  );
}

export default App;
