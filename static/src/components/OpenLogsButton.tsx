import { useQuery } from '@tanstack/react-query';
import { tauriFileSystem } from '../utils/tauri';

/** Opens the desktop app's log folder; its path is the tooltip. */
export default function OpenLogsButton({ onError }: { onError: (message: string) => void }) {
  const { data: folder } = useQuery({
    queryKey: ['logFolder'],
    queryFn: tauriFileSystem.getLogFolder,
    staleTime: Infinity,
  });
  return (
    <button
      type="button"
      className="cancel-button"
      title={folder ?? 'Open the folder the app writes its log to'}
      onClick={() => {
        tauriFileSystem.openLogFolder().catch((error: unknown) =>
          onError(`Failed to open the log folder: ${error instanceof Error ? error.message : String(error)}`)
        );
      }}
    >
      Open logs
    </button>
  );
}
