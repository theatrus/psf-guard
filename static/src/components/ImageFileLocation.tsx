import { useEffect, useState } from 'react';
import { isTauriApp, tauriFileSystem } from '../utils/tauri';

interface ImageFileLocationProps {
  dbId: string;
  filesystemPath: string | null;
  catalogPath?: string | null;
}

type ActionState = 'idle' | 'copied' | 'opening' | 'error';

export default function ImageFileLocation({
  dbId,
  filesystemPath,
  catalogPath,
}: ImageFileLocationProps) {
  const path = filesystemPath || catalogPath || null;
  const [actionState, setActionState] = useState<ActionState>('idle');
  const [error, setError] = useState<string | null>(null);
  const canReveal = isTauriApp() && !!filesystemPath;
  const pathState = filesystemPath
    ? { label: 'Resolved', className: 'file-path-resolved' }
    : catalogPath
      ? { label: 'Catalog only', className: 'file-path-catalog' }
      : { label: 'Unavailable', className: 'file-path-unavailable' };

  useEffect(() => {
    setActionState('idle');
    setError(null);
  }, [path]);

  const copyPath = async () => {
    if (!path) return;
    try {
      await navigator.clipboard.writeText(path);
      setActionState('copied');
      setError(null);
    } catch {
      setActionState('error');
      setError('Could not copy the path.');
    }
  };

  const showInFolder = async () => {
    if (!filesystemPath) return;
    setActionState('opening');
    setError(null);
    try {
      await tauriFileSystem.showImageInFolder(dbId, filesystemPath);
      setActionState('idle');
    } catch (cause) {
      setActionState('error');
      setError(cause instanceof Error ? cause.message : 'Could not open the folder.');
    }
  };

  return (
    <div className="image-file-location">
      <div className="image-file-heading">
        <span>File path</span>
        <span className={pathState.className}>{pathState.label}</span>
      </div>

      {path ? (
        <code className="image-file-path" data-testid="image-file-path" title={path}>
          {path}
        </code>
      ) : (
        <span className="image-file-unavailable">No file path is recorded.</span>
      )}

      {!filesystemPath && catalogPath && (
        <p className="image-file-note">
          This catalog path does not resolve in the configured image folders.
        </p>
      )}

      {path && (
        <div className="image-file-actions">
          <button type="button" onClick={copyPath}>
            {actionState === 'copied' ? 'Copied' : 'Copy path'}
          </button>
          {canReveal && (
            <button
              type="button"
              onClick={showInFolder}
              disabled={actionState === 'opening'}
            >
              {actionState === 'opening' ? 'Opening…' : 'Show in folder'}
            </button>
          )}
        </div>
      )}

      <div className="image-file-feedback" aria-live="polite">
        {error}
      </div>
    </div>
  );
}
