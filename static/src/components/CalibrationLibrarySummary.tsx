import { useQuery } from '@tanstack/react-query';
import { useState } from 'react';
import { apiClient } from '../api/client';
import CalibrationLibraryDialog from './CalibrationLibraryDialog';

interface CalibrationLibrarySummaryProps {
  dbId: string;
  dbName?: string;
  canManage?: boolean;
  onImport?: () => void;
}

export default function CalibrationLibrarySummary({
  dbId,
  dbName = dbId,
  canManage = false,
  onImport,
}: CalibrationLibrarySummaryProps) {
  const [showLibrary, setShowLibrary] = useState(false);
  const query = useQuery({
    queryKey: ['db', dbId, 'calibrations'],
    queryFn: () => apiClient.getCalibrationLibrary(dbId),
  });

  if (query.isLoading) {
    return <div className="calibration-summary muted">Calibration library: loading…</div>;
  }
  if (query.error) {
    return (
      <div className="calibration-summary calibration-summary-error">
        Calibration library could not be read.
      </div>
    );
  }

  const library = query.data;
  if (!library || library.frame_count === 0) {
    return (
      <>
        <div className="calibration-summary muted">
          <span>
            Calibration library: empty. Import folders containing bias, dark, dark-flat, or flat
            FITS frames to add them.
          </span>
          <button className="calibration-manage-button" onClick={() => setShowLibrary(true)}>
            {canManage ? 'Manage' : 'View'}
          </button>
        </div>
        {showLibrary && (
          <CalibrationLibraryDialog
            dbId={dbId}
            dbName={dbName}
            canManage={canManage}
            onClose={() => setShowLibrary(false)}
            onImport={onImport}
          />
        )}
      </>
    );
  }

  return (
    <>
      <div className="calibration-summary">
        <div className="calibration-summary-heading">
          <div className="calibration-summary-title">
            Calibration library · {library.frame_count} frame
            {library.frame_count === 1 ? '' : 's'}
            {library.master_count > 0 &&
              ` · ${library.master_count} cached master${library.master_count === 1 ? '' : 's'}`}
          </div>
          <button className="calibration-manage-button" onClick={() => setShowLibrary(true)}>
            {canManage ? 'Manage' : 'View'}
          </button>
        </div>
        {library.rigs.map((rig) => (
          <div className="calibration-rig" key={rig.rig_uuid}>
            <span>{rig.name}</span>
            <span className="muted">
              {rig.bias} bias · {rig.dark} dark · {rig.dark_flat} dark-flat · {rig.flat} flat
            </span>
          </div>
        ))}
      </div>
      {showLibrary && (
        <CalibrationLibraryDialog
          dbId={dbId}
          dbName={dbName}
          canManage={canManage}
          onClose={() => setShowLibrary(false)}
          onImport={onImport}
        />
      )}
    </>
  );
}
