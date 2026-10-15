import { useMutation, useQueryClient } from '@tanstack/react-query';
import { FolderSync } from 'lucide-react';
import { apiClient } from '../../api/client';

/**
 * The ⋯ menu's Rescan image folders: rebuilds the database's folder tree,
 * then its file index, now rather than when they next go stale (the server
 * rebuilds a tree older than five minutes on its own, and sync and automatic
 * import refresh it too). For files copied in by hand, so their frames'
 * previews find them at once. Progress shows in the header's activity chip.
 */
export default function RescanImageFolders({ dbId }: { dbId: string | null }) {
  const queryClient = useQueryClient();
  const rescan = useMutation({
    mutationFn: async () => {
      await apiClient.refreshDirectoryCache(dbId!);
      await apiClient.refreshFileCache(dbId!);
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['db', dbId] }),
    onError: (error) => console.error('Rescan of image folders failed:', error),
  });
  return (
    <button
      type="button"
      className="toolbar-menu-item"
      disabled={!dbId || rescan.isPending}
      title={rescan.isError ? 'The last rescan failed; try again' : 'Look for image files added since the last scan'}
      onClick={() => rescan.mutate()}
    >
      <FolderSync size={14} aria-hidden="true" /> {rescan.isPending ? 'Rescanning image folders…' : 'Rescan image folders'}
    </button>
  );
}
