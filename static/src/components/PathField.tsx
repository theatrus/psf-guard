import type { InputHTMLAttributes } from 'react';
import { isTauriApp, tauriFileSystem } from '../utils/tauri';

type PathFieldProps = Omit<InputHTMLAttributes<HTMLInputElement>, 'type' | 'value' | 'onChange'> & {
  value: string;
  onChange: (path: string) => void;
  /** What Browse… picks. */
  kind?: 'dir' | 'file';
  /** The dialog's title. */
  dialogTitle?: string;
  /** A picker of its own, such as one with file filters. */
  pick?: () => Promise<string | null>;
};

/**
 * A text field for a path on the server. In the desktop app, where the
 * server is this machine, a Browse… button opens the system dialog beside it.
 */
export default function PathField({
  value,
  onChange,
  kind = 'dir',
  dialogTitle,
  pick,
  disabled,
  ...input
}: PathFieldProps) {
  const browse = async () => {
    const picked = pick ? await pick() : await tauriFileSystem.pickPath({ title: dialogTitle, file: kind === 'file' });
    if (picked) onChange(picked);
  };
  return (
    <span className="path-field">
      <input
        {...input}
        type="text"
        spellCheck={false}
        value={value}
        disabled={disabled}
        onChange={(event) => onChange(event.target.value)}
      />
      {isTauriApp() && (
        <button type="button" className="browse-button" disabled={disabled} onClick={browse}>
          Browse…
        </button>
      )}
    </span>
  );
}
