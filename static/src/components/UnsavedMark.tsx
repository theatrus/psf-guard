import { useUnsavedChanges } from '../hooks/useUnsavedChanges';

/**
 * Registers a form's unsaved edits from where it renders, so a form whose
 * edits are known only after an early return can still say so: Settings
 * asks before closing while `dirty`. Renders nothing.
 */
export default function UnsavedMark({ label, dirty }: { label: string; dirty: boolean }) {
  useUnsavedChanges(label, dirty);
  return null;
}
