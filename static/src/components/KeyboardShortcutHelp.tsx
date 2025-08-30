import { useHotkeys } from 'react-hotkeys-hook';

interface KeyboardShortcutHelpProps {
  onClose: () => void;
}

export default function KeyboardShortcutHelp({ onClose }: KeyboardShortcutHelpProps) {
  useHotkeys('escape', onClose, [onClose]);

  const shortcuts = [
    { category: 'Navigation', items: [
      { key: 'K', description: 'Next image' },
      { key: 'J', description: 'Previous image' },
      { key: '→', description: 'Next image (alternative)' },
      { key: '←', description: 'Previous image (alternative)' },
      { key: 'Enter', description: 'Open image detail view' },
      { key: 'ESC', description: 'Close detail view / modal' },
    ]},
    { category: 'Grading', items: [
      { key: 'A', description: 'Accept image (or batch if multiple selected)' },
      { key: 'R', description: 'Reject image (or batch if multiple selected)' },
      { key: 'U', description: 'Unmark image (or batch if multiple selected)' },
    ]},
    { category: 'View Options', items: [
      { key: 'S', description: 'Toggle star detection overlay' },
      { key: 'P', description: 'Toggle PSF residual visualization' },
      { key: 'Z', description: 'Toggle image size' },
      { key: 'G', description: 'Cycle grouping mode (Filter → Date → Both)' },
      { key: '?', description: 'Show this help' },
    ]},
    { category: 'Zoom & Pan', items: [
      { key: '+ / =', description: 'Zoom in' },
      { key: '-', description: 'Zoom out' },
      { key: 'F', description: 'Fit to screen' },
      { key: '1', description: '100% zoom' },
      { key: '0', description: 'Reset zoom' },
      { key: 'Mouse Wheel', description: 'Zoom in/out toward cursor' },
      { key: 'Click & Drag', description: 'Pan image when zoomed' },
    ]},
    { category: 'Batch Selection', items: [
      { key: 'Shift+Click', description: 'Select range of images' },
      { key: 'Ctrl+Click', description: 'Toggle individual image selection' },
      { key: 'ESC', description: 'Clear all selections' },
    ]},
    { category: 'Undo/Redo', items: [
      { key: 'Ctrl+Z', description: 'Undo last grading action' },
      { key: 'Cmd+Z', description: 'Undo last grading action (Mac)' },
      { key: 'Ctrl+Y', description: 'Redo last grading action' },
      { key: 'Cmd+Y', description: 'Redo last grading action (Mac)' },
    ]},
  ];

  return (
    <div className="help-overlay" onClick={onClose}>
      <div className="help-modal" onClick={e => e.stopPropagation()}>
        <div className="help-header">
          <h2>Keyboard Shortcuts</h2>
          <button className="close-button" onClick={onClose}>×</button>
        </div>
        
        <div className="help-content">
          {shortcuts.map(section => (
            <div key={section.category} className="help-section">
              <h3>{section.category}</h3>
              <div className="shortcut-list">
                {section.items.map(item => (
                  <div key={item.key} className="shortcut-item">
                    <kbd>{item.key}</kbd>
                    <span>{item.description}</span>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
        
        <div className="help-footer">
          <p>Press <kbd>ESC</kbd> to close this dialog</p>
        </div>
      </div>
    </div>
  );
}