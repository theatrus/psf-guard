import { useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import AstroBinFilterMapDialog from './AstroBinFilterMapDialog';

interface Props {
  dbId: string;
  dbName?: string;
  canManage?: boolean;
}

/** One line per database in Settings: how many filters its map names, and the way in. */
export default function AstroBinFilterSummary({ dbId, dbName = dbId, canManage = false }: Props) {
  const [open, setOpen] = useState(false);
  const map = useQuery({
    queryKey: ['db', dbId, 'astrobin-filters'],
    queryFn: () => apiClient.getAstroBinFilters(dbId),
  });
  const entries = map.data?.entries ?? [];
  const names = new Set(entries.map((entry) => entry.filter_name));

  return (
    <>
      <div className="calibration-summary astrobin-filter-summary">
        <div className="calibration-summary-heading">
          <div className={entries.length === 0 ? 'muted' : 'calibration-summary-title'}>
            {map.isError
              ? 'AstroBin filters could not be read.'
              : entries.length === 0
                ? 'AstroBin filters: none named yet; the export uses the server-wide defaults.'
                : `AstroBin filters · ${names.size} name${names.size === 1 ? '' : 's'}, ${entries.length} ${entries.length === 1 ? 'entry' : 'entries'}`}
          </div>
          <button className="calibration-manage-button" onClick={() => setOpen(true)}>
            {canManage ? 'Edit filters' : 'View filters'}
          </button>
        </div>
      </div>
      {open && (
        <AstroBinFilterMapDialog
          dbId={dbId}
          dbName={dbName}
          canManage={canManage}
          onClose={() => setOpen(false)}
        />
      )}
    </>
  );
}
