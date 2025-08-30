import { useEffect, useState } from 'react';
import { useHotkeys } from 'react-hotkeys-hook';
import type { GradingAction } from '../hooks/useUndoRedo';

interface UndoRedoToolbarProps {
  canUndo: boolean;
  canRedo: boolean;
  isProcessing: boolean;
  undoStackSize: number;
  redoStackSize: number;
  onUndo: () => Promise<boolean>;
  onRedo: () => Promise<boolean>;
  getLastAction: () => GradingAction | null;
  getNextRedoAction: () => GradingAction | null;
  className?: string;
}

export default function UndoRedoToolbar({
  canUndo,
  canRedo,
  isProcessing,
  undoStackSize,
  redoStackSize,
  onUndo,
  onRedo,
  getLastAction,
  getNextRedoAction,
  className = '',
}: UndoRedoToolbarProps) {
  const [showFeedback, setShowFeedback] = useState<'undo' | 'redo' | null>(null);

  // Keyboard shortcuts
  useHotkeys('ctrl+z,cmd+z', (event) => {
    event.preventDefault();
    if (canUndo && !isProcessing) {
      handleUndo();
    }
  }, [canUndo, isProcessing]);

  useHotkeys('ctrl+y,cmd+y,ctrl+shift+z,cmd+shift+z', (event) => {
    event.preventDefault();
    if (canRedo && !isProcessing) {
      handleRedo();
    }
  }, [canRedo, isProcessing]);

  const handleUndo = async () => {
    if (!canUndo || isProcessing) return;
    
    const success = await onUndo();
    if (success) {
      setShowFeedback('undo');
      setTimeout(() => setShowFeedback(null), 2000);
    }
  };

  const handleRedo = async () => {
    if (!canRedo || isProcessing) return;
    
    const success = await onRedo();
    if (success) {
      setShowFeedback('redo');
      setTimeout(() => setShowFeedback(null), 2000);
    }
  };

  const lastAction = getLastAction();
  const nextRedoAction = getNextRedoAction();

  const formatActionDescription = (action: GradingAction) => {
    return action.description;
  };

  const formatTime = (timestamp: number) => {
    const now = Date.now();
    const diff = now - timestamp;
    
    if (diff < 60000) { // Less than 1 minute
      return 'just now';
    } else if (diff < 3600000) { // Less than 1 hour
      const minutes = Math.floor(diff / 60000);
      return `${minutes}m ago`;
    } else {
      const hours = Math.floor(diff / 3600000);
      return `${hours}h ago`;
    }
  };

  // Clear feedback when actions change
  useEffect(() => {
    setShowFeedback(null);
  }, [undoStackSize, redoStackSize]);

  return (
    <div className={`undo-redo-toolbar ${className}`}>
      <div className="undo-redo-buttons">
        <button
          className={`undo-button ${!canUndo || isProcessing ? 'disabled' : ''}`}
          onClick={handleUndo}
          disabled={!canUndo || isProcessing}
          title={lastAction ? `Undo: ${formatActionDescription(lastAction)} (Ctrl+Z)` : 'Nothing to undo (Ctrl+Z)'}
        >
          <span className="undo-icon">↶</span>
          <span className="button-text">Undo</span>
          {undoStackSize > 0 && (
            <span className="stack-count">{undoStackSize}</span>
          )}
        </button>

        <button
          className={`redo-button ${!canRedo || isProcessing ? 'disabled' : ''}`}
          onClick={handleRedo}
          disabled={!canRedo || isProcessing}
          title={nextRedoAction ? `Redo: ${formatActionDescription(nextRedoAction)} (Ctrl+Y)` : 'Nothing to redo (Ctrl+Y)'}
        >
          <span className="redo-icon">↷</span>
          <span className="button-text">Redo</span>
          {redoStackSize > 0 && (
            <span className="stack-count">{redoStackSize}</span>
          )}
        </button>
      </div>

      {/* Action descriptions */}
      <div className="action-descriptions">
        {lastAction && (
          <div className="last-action">
            <span className="action-label">Last:</span>
            <span className="action-desc">{formatActionDescription(lastAction)}</span>
            <span className="action-time">({formatTime(lastAction.timestamp)})</span>
          </div>
        )}
      </div>

      {/* Feedback messages */}
      {showFeedback && (
        <div className={`feedback-message ${showFeedback}`}>
          {showFeedback === 'undo' && lastAction && (
            <span>Undid: {formatActionDescription(lastAction)}</span>
          )}
          {showFeedback === 'redo' && nextRedoAction && (
            <span>Redid: {formatActionDescription(nextRedoAction)}</span>
          )}
        </div>
      )}

      {isProcessing && (
        <div className="processing-indicator">
          <span className="spinner"></span>
          <span>Processing...</span>
        </div>
      )}
    </div>
  );
}