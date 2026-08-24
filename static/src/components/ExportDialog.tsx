import { useState } from 'react';
import { apiClient } from '../api/client';
import type { ExportLayout } from '../api/types';
import Dialog from './Dialog';

/** One export the user has asked for, awaiting its layout choice. */
export interface ExportRequest {
  dbId: string;
  scope: { project_id?: number; target_id?: number };
  label: string;
  /**
   * Which affordance applies: `local` picks a folder through the native
   * picker (desktop), `server` runs in the server's export directory,
   * `download` streams a zip.
   */
  kind: 'local' | 'server' | 'download';
}

interface ExportDialogProps {
  request: ExportRequest;
  /** What the layout choice starts from, per the settings panel. */
  defaultLayout: ExportLayout;
  busy: boolean;
  onClose: () => void;
  /** Runs the local or server export with the chosen layout. */
  onConfirm: (layout: ExportLayout) => void;
}

const LAYOUT_HELP: Record<ExportLayout, string> = {
  standard:
    'Grouped by target: <target>/LIGHT/<filter>, with BIAS, DARK and DARKFLAT at the root.',
  wbpp: 'One folder per frame type, with dark flats among the darks, ready for WeightedBatchPreprocessing. Includes run-wbpp scripts.',
};

/**
 * The layout choice made at export time. Every export affordance funnels
 * through here so the choice is per export, not a page-wide mode.
 */
export default function ExportDialog({
  request,
  defaultLayout,
  busy,
  onClose,
  onConfirm,
}: ExportDialogProps) {
  const [layout, setLayout] = useState<ExportLayout>(defaultLayout);

  const confirmLabel =
    request.kind === 'local'
      ? 'Choose folder…'
      : request.kind === 'server'
        ? 'Start export'
        : 'Download zip';

  return (
    <Dialog
      open
      title={`Export ${request.label}`}
      onClose={onClose}
      className="export-dialog"
      footer={
        <>
          <button type="button" className="header-button" onClick={onClose}>
            Cancel
          </button>
          {request.kind === 'download' ? (
            <a
              className="action-button export-dialog-download"
              href={apiClient.exportDownloadUrl(request.dbId, {
                ...request.scope,
                layout,
              })}
              onClick={onClose}
            >
              {confirmLabel}
            </a>
          ) : (
            <button
              type="button"
              className="action-button"
              disabled={busy}
              onClick={() => onConfirm(layout)}
            >
              {confirmLabel}
            </button>
          )}
        </>
      }
    >
      <p className="export-dialog-scope">
        Accepted lights and their calibration frames; rejects excluded.
      </p>
      <fieldset className="export-layout-options">
        <legend>Layout</legend>
        {(['standard', 'wbpp'] as const).map((option) => (
          <label key={option} className="export-layout-option">
            <input
              type="radio"
              name="export-layout"
              value={option}
              checked={layout === option}
              onChange={() => setLayout(option)}
            />
            <span>
              <strong>{option === 'wbpp' ? 'WBPP' : 'Grouped by target'}</strong>
              <small>{LAYOUT_HELP[option]}</small>
            </span>
          </label>
        ))}
      </fieldset>
    </Dialog>
  );
}
