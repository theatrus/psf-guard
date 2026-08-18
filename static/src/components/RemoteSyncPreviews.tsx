import { useState } from 'react';
import { useQueries, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { SchedulerSyncPreviewListEntry } from '../api/types';

const KIND_LABELS: Record<string, string> = {
  merge: 'Merge',
  push_planning: 'Planning push',
  push_grades: 'Grades push',
};

function expiresIn(expiresAt: number): string {
  const seconds = expiresAt - Math.floor(Date.now() / 1000);
  if (seconds <= 0) return 'expired';
  if (seconds < 90) return `${seconds}s`;
  return `${Math.round(seconds / 60)} min`;
}

/**
 * Every staged transfer parked on the server, per catalog — including
 * previews a remote client (the N.I.N.A. plugin) created and never
 * applied. Without this listing those previews sit invisible until they
 * expire; the operator can now review, apply, refresh, or discard them.
 */
export default function RemoteSyncPreviews({
  databases,
  disabled = false,
}: {
  databases: Array<{ id: string; name: string }>;
  disabled?: boolean;
}) {
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState<string | null>(null);
  const [message, setMessage] = useState('');

  const previewQueries = useQueries({
    queries: databases.map((db) => ({
      queryKey: ['db', db.id, 'sync-preview-list'] as const,
      queryFn: () => apiClient.listDatabaseSyncPreviews(db.id),
      refetchInterval: 30_000,
    })),
  });

  const rows: Array<{ dbId: string; dbName: string; entry: SchedulerSyncPreviewListEntry }> =
    databases.flatMap((db, index) =>
      (previewQueries[index]?.data ?? []).map((entry) => ({
        dbId: db.id,
        dbName: db.name,
        entry,
      }))
    );

  const reload = (dbId: string) =>
    queryClient.invalidateQueries({ queryKey: ['db', dbId, 'sync-preview-list'] });

  const act = async (
    label: string,
    key: string,
    dbId: string,
    action: () => Promise<unknown>
  ) => {
    setBusy(key);
    setMessage('');
    try {
      await action();
      setMessage(`${label} succeeded`);
      await reload(dbId);
      // An apply changes the catalog itself; refresh its views too.
      queryClient.invalidateQueries({ queryKey: ['db', dbId] });
    } catch (error) {
      setMessage(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(null);
    }
  };

  // After applying or discarding the last preview the table goes away,
  // but the outcome message must survive that render or the user never
  // sees whether their click worked.
  if (rows.length === 0 && !message) {
    return null;
  }

  return (
    <div className="remote-sync-previews">
      <h4>Staged previews</h4>
      <p className="remote-sync-previews-hint">
        Transfers waiting for review — including pushes a remote client
        staged. Nothing changes the catalog until Apply.
      </p>
      {rows.length > 0 && (
      <table>
        <thead>
          <tr>
            <th>Catalog</th>
            <th>Operation</th>
            <th>From</th>
            <th>Changes</th>
            <th>Expires</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {rows.map(({ dbId, dbName, entry }) => {
            const key = `${dbId}:${entry.preview_id}`;
            return (
              <tr key={key}>
                <td>{dbName}</td>
                <td>{KIND_LABELS[entry.kind] ?? entry.kind}</td>
                <td>{entry.source}</td>
                <td>
                  {entry.result.total_inserted} new · {entry.result.total_updated} updated
                  {entry.result.grades
                    ? ` · ${entry.result.grade_filled + entry.result.grade_preserved} grades`
                    : ''}
                </td>
                <td>{expiresIn(entry.expires_at)}</td>
                <td className="remote-sync-preview-actions">
                  <button
                    disabled={disabled || busy !== null}
                    onClick={() =>
                      act('Apply', key, dbId, () =>
                        apiClient.applyDatabaseSyncPreview(dbId, entry.preview_id)
                      )
                    }
                  >
                    {busy === key ? 'Working…' : 'Apply'}
                  </button>
                  <button
                    disabled={disabled || busy !== null}
                    onClick={() =>
                      act('Refresh', key, dbId, () =>
                        apiClient.refreshDatabaseSyncPreview(dbId, entry.preview_id)
                      )
                    }
                  >
                    Refresh
                  </button>
                  <button
                    disabled={disabled || busy !== null}
                    onClick={() =>
                      act('Discard', key, dbId, () =>
                        apiClient.deleteDatabaseSyncPreview(dbId, entry.preview_id)
                      )
                    }
                  >
                    Discard
                  </button>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
      )}
      {message && <p className="remote-sync-previews-status">{message}</p>}
    </div>
  );
}
