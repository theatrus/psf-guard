import { Redo2, Undo2 } from 'lucide-react';

interface PhoneUndoItemsProps {
  canUndo: boolean;
  canRedo: boolean;
  isProcessing: boolean;
  onUndo: () => void;
  onRedo: () => void;
}

/**
 * Undo and Redo as items of a toolbar's ⋯ menu, shown only on a phone,
 * where the toolbar leaves its own undo buttons out to keep to one row.
 */
export default function PhoneUndoItems({ canUndo, canRedo, isProcessing, onUndo, onRedo }: PhoneUndoItemsProps) {
  return (
    <>
      <button
        type="button"
        className="toolbar-menu-item toolbar-phone-only"
        disabled={!canUndo || isProcessing}
        onClick={onUndo}
      >
        <Undo2 size={14} aria-hidden="true" /> Undo
      </button>
      <button
        type="button"
        className="toolbar-menu-item toolbar-phone-only"
        disabled={!canRedo || isProcessing}
        onClick={onRedo}
      >
        <Redo2 size={14} aria-hidden="true" /> Redo
      </button>
    </>
  );
}
