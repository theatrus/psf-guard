import { useQuery } from '@tanstack/react-query';
import { useLocation, useNavigate } from 'react-router-dom';
import { apiClient } from '../api/client';
import { calibrationPageHref } from './calibration/calibrationLinks';

interface CalibrationLibrarySummaryProps {
  dbId: string;
  canManage?: boolean;
  /** Called when the button leaves for the calibration page, so a modal
   *  around this summary can close. */
  onOpen?: () => void;
}

export default function CalibrationLibrarySummary({
  dbId,
  canManage = false,
  onOpen,
}: CalibrationLibrarySummaryProps) {
  const navigate = useNavigate();
  const location = useLocation();
  const query = useQuery({
    queryKey: ['db', dbId, 'calibrations'],
    queryFn: () => apiClient.getCalibrationLibrary(dbId),
  });
  const open = () => {
    onOpen?.();
    navigate(calibrationPageHref(location.search, dbId));
  };
  const button = (
    <button className="calibration-manage-button" onClick={open}>
      {canManage ? 'Manage' : 'View'}
    </button>
  );

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
      <div className="calibration-summary muted">
        <span>
          Calibration library: empty. Import folders containing bias, dark, dark-flat, or flat
          FITS frames to add them.
        </span>
        {button}
      </div>
    );
  }

  return (
    <div className="calibration-summary">
      <div className="calibration-summary-heading">
        <div className="calibration-summary-title">
          Calibration library · {library.frame_count} frame
          {library.frame_count === 1 ? '' : 's'}
          {library.master_count > 0 &&
            ` · ${library.master_count} cached master${library.master_count === 1 ? '' : 's'}`}
        </div>
        {button}
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
  );
}
