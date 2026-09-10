import { useEffect, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Layers, RotateCcw } from 'lucide-react';
import { apiClient } from '../api/client';
import type { CalibrationMasterSource, StackCalibrationMaster } from '../api/types';
import StackPreviewInspector from './StackPreviewInspector';

interface Inspection {
  dbId: string;
  source: CalibrationMasterSource;
  title: string;
  label: string;
}

const kindLabels: Record<StackCalibrationMaster['kind'], string> = {
  bias: 'Bias', dark: 'Dark', dark_flat: 'Dark-flat', flat: 'Flat',
};

function usageLabel(master: StackCalibrationMaster): string {
  return master.usages.map((usage) => `${usage.channel} / Session ${usage.session}`).join(', ');
}

function stretchUrl(url: string | null, midtone: number, shadow: number): string | null {
  if (!url) return null;
  const [path, query = ''] = url.split('?');
  const params = new URLSearchParams(query);
  params.set('midtone', String(midtone));
  params.set('shadow', String(shadow));
  return `${path}?${params}`;
}

export function CalibrationMasterInspector({ dbId, source, title, label, onClose }: Inspection & {
  onClose: () => void;
}) {
  const catalog = useQuery({
    queryKey: ['db', dbId, 'stack-calibration-masters', source],
    queryFn: () => apiClient.getStackCalibrationMasters(dbId, source),
    retry: false,
  });
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [draftStretch, setDraftStretch] = useState({ midtone: 0.2, shadow: -2.8 });
  const [stretch, setStretch] = useState(draftStretch);
  useEffect(() => {
    const timer = setTimeout(() => setStretch(draftStretch), 250);
    return () => clearTimeout(timer);
  }, [draftStretch]);
  const masters = catalog.data?.masters ?? [];
  const master = masters.find((entry) => entry.id === selectedId)
    ?? masters.find((entry) => entry.available && entry.kind === 'flat')
    ?? masters.find((entry) => entry.available)
    ?? masters[0];
  const masterLabel = master ? kindLabels[master.kind] : 'Calibration masters';
  const imageUrl = master?.available
    ? stretchUrl(master.original_preview_url, stretch.midtone, stretch.shadow)
    : null;
  const summary = master ? [
    ...(master.source_count != null ? [`${master.source_count} source frames`] : []),
    ...(master.rejection_method ? [master.rejection_method] : []),
    ...(master.masked_samples != null ? [`${master.masked_samples.toLocaleString()} masked samples`] : []),
    ...(master.rejected_samples != null ? [`${master.rejected_samples.toLocaleString()} rejected samples`] : []),
    ...(master.minimum_clean_samples != null && master.maximum_clean_samples != null
      ? [`${master.minimum_clean_samples}-${master.maximum_clean_samples} retained samples / pixel`] : []),
  ] : [];
  const emptyMessage = catalog.isPending ? 'Loading calibration masters...'
    : catalog.isError ? 'Calibration masters could not be loaded.'
      : !master ? 'No calibration master files were recorded for this stack.'
        : master.unavailable_reason || 'This calibration master is unavailable.';

  return (
    <StackPreviewInspector
      className="calibration-master-inspector"
      eyebrow="Applied calibration"
      title={title}
      label={`${label} / ${masterLabel}`}
      summary={[]}
      imageUrl={imageUrl}
      imageIdentity={master ? `${dbId}:${source.artifactRevision}:${master.id}` : undefined}
      fitsUrl={master?.available ? master.fits_url : null}
      imageAlt={`${masterLabel} master for ${title}`}
      downloadLabel="Download master FITS"
      closeLabel="Close calibration master inspector"
      loadingMessage="Loading calibration master..."
      errorMessage="The calibration master preview could not be loaded."
      emptyMessage={emptyMessage}
      asyncPreview={master && imageUrl ? {
        dbId,
        descriptor: {
          kind: 'calibration_master', source, masterId: master.id, size: 'original', ...stretch,
        },
      } : undefined}
      controls={(
        <div className="calibration-master-controls">
          {masters.length > 0 && (
            <div className="calibration-master-options">
              <label className="calibration-master-selector">
                Master
                <select value={master.id} onChange={(event) => setSelectedId(event.target.value)}>
                  {masters.map((entry) => (
                    <option key={entry.id} value={entry.id}>
                      {kindLabels[entry.kind]} / {usageLabel(entry)}{entry.available ? '' : ' (unavailable)'}
                    </option>
                  ))}
                </select>
              </label>
              <label className="calibration-master-stretch">
                Midtone <output>{draftStretch.midtone.toFixed(2)}</output>
                <input type="range" aria-label="Master midtone" min="0.01" max="0.99" step="0.01"
                  value={draftStretch.midtone} disabled={!imageUrl}
                  onChange={(event) => setDraftStretch((current) => ({ ...current, midtone: Number(event.target.value) }))} />
              </label>
              <label className="calibration-master-stretch">
                Shadows <output>{draftStretch.shadow.toFixed(1)}</output>
                <input type="range" aria-label="Master shadows" min="-10" max="0" step="0.1"
                  value={draftStretch.shadow} disabled={!imageUrl}
                  onChange={(event) => setDraftStretch((current) => ({ ...current, shadow: Number(event.target.value) }))} />
              </label>
              <button type="button" className="stack-preview-card-action" title="Reset master stretch"
                aria-label="Reset master stretch" disabled={!imageUrl}
                onClick={() => setDraftStretch({ midtone: 0.2, shadow: -2.8 })}>
                <RotateCcw size={16} aria-hidden="true" />
              </button>
            </div>
          )}
          {master && (
            <div className="calibration-master-provenance">
              <strong>{master.label}</strong>
              {master.usages.map((usage, index) => (
                <span key={`${usage.channel}:${usage.session}:${index}`}>
                  {usage.channel} / Session {usage.session}: {usage.lights} light frames
                  {usage.estimated_pedestal_adu != null && ` / Estimated pedestal ${usage.estimated_pedestal_adu.toFixed(1)} ADU`}
                </span>
              ))}
              <div className="calibration-master-statistics">
                {summary.map((item) => <span key={item}>{item}</span>)}
              </div>
            </div>
          )}
          {catalog.data?.notes.map((note, index) => <p className="calibration-master-note" key={index}>{note}</p>)}
          {catalog.isError && (
            <div className="calibration-master-error" role="alert">
              <span>{catalog.error instanceof Error ? catalog.error.message : 'Unable to read calibration provenance.'}</span>
              <button type="button" className="stack-preview-card-action" onClick={() => void catalog.refetch()}
                disabled={catalog.isFetching} title="Retry loading calibration masters" aria-label="Retry loading calibration masters">
                <RotateCcw size={16} aria-hidden="true" />
              </button>
            </div>
          )}
        </div>
      )}
      onClose={onClose}
    />
  );
}

export default function CalibrationMasterButton(props: Inspection) {
  const [inspection, setInspection] = useState<Inspection | null>(null);
  return (
    <>
      <button className="stack-preview-card-action calibration-master-button" type="button"
        title="Inspect applied calibration masters" aria-label={`Inspect calibration masters for ${props.label}`}
        onClick={() => setInspection({ ...props })}>
        <Layers size={15} aria-hidden="true" /> Masters
      </button>
      {inspection && <CalibrationMasterInspector {...inspection} onClose={() => setInspection(null)} />}
    </>
  );
}
