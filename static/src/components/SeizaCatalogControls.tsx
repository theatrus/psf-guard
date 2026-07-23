import { useEffect, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  AstrometryResourceCapability,
  CatalogInstallPreset,
} from '../api/types';

const PRESETS: Array<{
  value: CatalogInstallPreset;
  label: string;
  description: string;
}> = [
  {
    value: 'blind_deep',
    label: 'Blind solving (recommended)',
    description: 'Deep Gaia stars, blind index, objects, minor bodies, and transients.',
  },
  {
    value: 'solver_lite',
    label: 'Compact hinted solving',
    description: 'Tycho-2 stars and overlays for systems with reliable coordinates.',
  },
  {
    value: 'solver_gaia',
    label: 'Dense hinted solving',
    description: 'A denser Gaia catalog for narrow or crowded fields; no blind index.',
  },
  {
    value: 'blind_deep_gaia20',
    label: 'Deepest blind solving',
    description: 'G≤20 Gaia data plus the blind index. This adds about 9 GB.',
  },
];

const RESOURCE_LABELS: Array<[string, string]> = [
  ['objects', 'Objects and outlines'],
  ['stars', 'Plate-solving stars'],
  ['blind_index', 'Blind-solve index'],
  ['star_identifiers', 'Star identifiers'],
  ['transients', 'Transient objects'],
  ['minor_bodies', 'Minor bodies'],
];

function formatBytes(bytes?: number): string {
  if (bytes === undefined) return '';
  if (bytes < 1024) return `${bytes} B`;
  const units = ['KiB', 'MiB', 'GiB', 'TiB'];
  let value = bytes / 1024;
  let unit = units[0];
  for (let index = 1; index < units.length && value >= 1024; index += 1) {
    value /= 1024;
    unit = units[index];
  }
  return `${value >= 10 ? value.toFixed(0) : value.toFixed(1)} ${unit}`;
}

function resourceDetail(resource: AstrometryResourceCapability): string {
  if (resource.status === 'available') {
    return [resource.format, formatBytes(resource.size_bytes)].filter(Boolean).join(' · ');
  }
  return resource.error || resource.status.replace('_', ' ');
}

export default function SeizaCatalogControls() {
  const queryClient = useQueryClient();
  const [preset, setPreset] = useState<CatalogInstallPreset>('blind_deep');
  const capabilities = useQuery({
    queryKey: ['astrometry', 'capabilities'],
    queryFn: apiClient.getAstrometryCapabilities,
  });
  const installStatus = useQuery({
    queryKey: ['astrometry', 'catalog-install'],
    queryFn: apiClient.getCatalogInstallStatus,
    refetchInterval: (query) => (query.state.data?.progress.running ? 1000 : false),
    refetchIntervalInBackground: true,
  });
  const install = useMutation({
    mutationFn: apiClient.installCatalogs,
    onSuccess: (status) => {
      queryClient.setQueryData(['astrometry', 'catalog-install'], status);
    },
  });
  const validation = useMutation({
    mutationFn: apiClient.validateAstrometryCatalogs,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['astrometry', 'capabilities'] });
    },
  });

  const running = installStatus.data?.progress.running ?? false;
  const wasRunning = useRef(false);
  useEffect(() => {
    if (wasRunning.current && !running) {
      queryClient.invalidateQueries({ queryKey: ['astrometry', 'capabilities'] });
    }
    wasRunning.current = running;
  }, [queryClient, running]);

  const progress = installStatus.data?.progress;
  const selectedPreset = PRESETS.find((candidate) => candidate.value === preset)!;
  const resources = capabilities.data?.resources;

  return (
    <section className="settings-section catalog-settings" aria-labelledby="seiza-catalog-title">
      <div className="catalog-heading">
        <div>
          <h3 id="seiza-catalog-title">Seiza Catalogs</h3>
          <p>
            Install the sky data used for object overlays and plate solving. Downloads are
            verified, cached, and safe to retry.
          </p>
        </div>
        {capabilities.data && (
          <div className="catalog-feature-summary" aria-label="Catalog feature status">
            <span className={capabilities.data.features.object_association ? 'ready' : ''}>
              {capabilities.data.features.object_association ? '✓' : '○'} Overlays
            </span>
            <span className={capabilities.data.features.hinted_solve ? 'ready' : ''}>
              {capabilities.data.features.hinted_solve ? '✓' : '○'} Hinted solve
            </span>
            <span className={capabilities.data.features.blind_solve ? 'ready' : ''}>
              {capabilities.data.features.blind_solve ? '✓' : '○'} Blind solve
            </span>
          </div>
        )}
      </div>

      {capabilities.isError && (
        <div className="catalog-error">
          Could not inspect catalogs: {capabilities.error.message}
        </div>
      )}

      {resources && (
        <div className="catalog-resource-grid">
          {RESOURCE_LABELS.map(([key, label]) => {
            const resource = resources[key as keyof typeof resources];
            return (
              <div className="catalog-resource" key={key}>
                <span
                  className={`catalog-status-dot catalog-status-${resource.status}`}
                  aria-hidden="true"
                />
                <span>
                  <strong>{label}</strong>
                  <small>{resourceDetail(resource)}</small>
                </span>
              </div>
            );
          })}
        </div>
      )}

      <div className="catalog-install-controls">
        <label htmlFor="catalog-preset">Catalog package</label>
        <select
          id="catalog-preset"
          value={preset}
          onChange={(event) => setPreset(event.target.value as CatalogInstallPreset)}
          disabled={running || install.isPending || validation.isPending}
        >
          {PRESETS.map((candidate) => (
            <option key={candidate.value} value={candidate.value}>
              {candidate.label}
            </option>
          ))}
        </select>
        <small>{selectedPreset.description}</small>
        <div className="catalog-actions">
          <button
            className="save-button"
            onClick={() => install.mutate(preset)}
            disabled={running || install.isPending || validation.isPending}
          >
            {running || install.isPending ? 'Installing…' : 'Install / update catalogs'}
          </button>
          <button
            className="browse-button"
            onClick={() => validation.mutate()}
            disabled={running || install.isPending || validation.isPending}
          >
            {validation.isPending ? 'Validating…' : 'Validate installed files'}
          </button>
        </div>
      </div>

      {progress && progress.phase !== 'idle' && (
        <div
          className={`catalog-progress catalog-progress-${progress.phase}`}
          role="status"
          aria-live="polite"
        >
          <strong>{progress.message}</strong>
          {progress.running && progress.files_total > 0 && (
            <span>
              {progress.files_completed}/{progress.files_total} files
            </span>
          )}
          {progress.bytes_total !== undefined && progress.bytes_completed !== undefined && (
            <>
              <progress value={progress.bytes_completed} max={progress.bytes_total} />
              <small>
                {formatBytes(progress.bytes_completed)} of {formatBytes(progress.bytes_total)}
                {progress.written_bytes !== undefined &&
                  progress.written_bytes !== progress.bytes_completed &&
                  ` · ${formatBytes(progress.written_bytes)} written`}
              </small>
            </>
          )}
          {progress.output_dir && <code>{progress.output_dir}</code>}
        </div>
      )}

      {install.isError && (
        <div className="catalog-error">Could not start installation: {install.error.message}</div>
      )}

      {validation.data && (
        <div className={validation.data.all_configured_valid ? 'catalog-valid' : 'catalog-error'}>
          {validation.data.all_configured_valid
            ? 'All configured catalog files passed validation.'
            : 'Some catalog files are missing or invalid. See the status above.'}
        </div>
      )}
      {validation.isError && (
        <div className="catalog-error">Validation failed: {validation.error.message}</div>
      )}

      <p className="catalog-footnote">
        Packages are additive. PSF Guard keeps existing catalog files and uses the deepest
        compatible star catalog it finds.
      </p>
    </section>
  );
}
