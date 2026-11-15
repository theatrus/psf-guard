import { useEffect, useRef, useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import { useRcAstroCapabilities } from './rcAstroSchema';
import {
  NOISE_MODEL_PROVIDER,
  PROCESSING_CATEGORIES,
  PROCESSING_TOOLS_FIELD,
  processingCapabilitiesKey,
} from './processingTools';
import type {
  NoiseModelDownloadPart,
  ProcessingCapabilities,
  ProcessingProvider,
} from '../api/types';

function megabytes(bytes: number): string {
  return `${(bytes / (1024 * 1024)).toFixed(bytes < 10 * 1024 * 1024 ? 1 : 0)} MB`;
}

const stateLabel: Record<ProcessingProvider['state'], string> = {
  ready: 'Ready',
  not_licensed: 'Not licensed',
  not_installed: 'Not installed',
};

/** Where a provider is installed, as "this server, studio". */
function machines(provider: ProcessingProvider): string {
  return [...(provider.this_server ? ['this server'] : []), ...provider.workers].join(', ');
}

/**
 * Settings › Stacking › Processing tools: every provider of a processing
 * step with its state, and what a person can do about it. The RC-Astro tools
 * show their license state and version. Seiza's noise model and the ONNX
 * Runtime it needs have downloads, which start only on a click, and a path
 * for an ONNX Runtime the person installed.
 */
export default function ProcessingToolsSettings({
  canManage,
  focus = null,
}: {
  /** Whether this person may set a path on the server. */
  canManage: boolean;
  /** `processing_tools` when a processing card sent the person here. */
  focus?: string | null;
}) {
  const queryClient = useQueryClient();
  const capabilities = useRcAstroCapabilities({ refetchOnMount: 'always' });
  const section = useRef<HTMLElement>(null);
  useEffect(() => {
    if (focus === PROCESSING_TOOLS_FIELD) section.current?.scrollIntoView?.({ block: 'start' });
  }, [focus]);

  const data = capabilities.data;
  const seiza = data?.seiza;
  const [path, setPath] = useState<string | null>(null);
  const savedPath = seiza?.onnxruntime ?? '';
  const shownPath = path ?? savedPath;

  const refresh = () => queryClient.invalidateQueries({ queryKey: processingCapabilitiesKey });
  const download = useMutation({
    // Asking for ONNX Runtime alone when it is installed asks for it again.
    mutationFn: (parts: NoiseModelDownloadPart[]) => apiClient.downloadNoiseModel(
      parts,
      parts.length === 1 && parts[0] === 'runtime' && !!capabilities.data?.seiza?.installed.runtime_version,
    ),
    onSuccess: (progress) => {
      queryClient.setQueryData<ProcessingCapabilities>(processingCapabilitiesKey, (current) =>
        current?.seiza ? { ...current, seiza: { ...current.seiza, download: progress } } : current);
      return refresh();
    },
  });
  const stop = useMutation({ mutationFn: apiClient.stopNoiseModelDownload, onSuccess: refresh });
  const savePath = useMutation({
    mutationFn: (value: string) => apiClient.updateNoiseModelSettings(value),
    onSuccess: (saved) => {
      queryClient.setQueryData<ProcessingCapabilities>(processingCapabilitiesKey, (current) =>
        current ? { ...current, seiza: saved } : current);
      setPath(null);
      // A new path can change whether the noise model is ready.
      return refresh();
    },
  });

  const progress = seiza?.download;
  const running = !!progress?.running;
  const busy = running || download.isPending;
  const installed = seiza?.installed;
  const runtimeReady = !!installed?.runtime_version;
  const usesOwnRuntime = !!seiza?.onnxruntime;
  const missing: NoiseModelDownloadPart[] = seiza ? [
    ...(!runtimeReady && !usesOwnRuntime && seiza.runtime_download ? ['runtime' as const] : []),
    ...(!installed?.model ? ['model' as const] : []),
  ] : [];
  const providers = PROCESSING_CATEGORIES.flatMap(({ category, label }) =>
    (data?.providers ?? [])
      .filter((provider) => provider.category === category)
      .map((provider) => ({ provider, label })));

  return (
    <section className="settings-section catalog-settings" aria-labelledby="processing-tools-title" ref={section}>
      <div className="catalog-heading">
        <div>
          <h3 id="processing-tools-title">Processing tools</h3>
          <p>
            The tools that sharpen a stack, reduce its noise and remove its stars.
            {seiza && ' PSF Guard downloads ONNX Runtime and the noise model only when you click Download.'}
          </p>
        </div>
      </div>

      {capabilities.isError && !data && (
        <div className="catalog-error">Could not check the processing tools: {capabilities.error.message}</div>
      )}
      {data?.error && <div className="catalog-error">{data.error}</div>}
      {data && !providers.length && !seiza && (
        <p className="catalog-footnote">No processing tools are installed on this server or a connected worker.</p>
      )}

      {providers.map(({ provider, label }) => {
        const isNoiseModel = !!seiza && provider.provider === NOISE_MODEL_PROVIDER;
        const where = machines(provider);
        return (
          <div className="catalog-optional" data-provider={provider.provider} key={provider.provider}>
            <div>
              <strong>{provider.name}</strong>
              <small>
                {label}
                {provider.version ? ` · ${provider.version}` : ''}
                {where ? ` · on ${where}` : ''}
              </small>
              {provider.state !== 'ready' && provider.reason && <small>{provider.reason}</small>}
            </div>
            <span className="catalog-optional-state" data-installed={provider.state === 'ready' ? 'true' : 'false'}>
              {stateLabel[provider.state]}
            </span>
            {isNoiseModel ? (
              <button type="button" className="browse-button" disabled={busy}
                aria-label={installed?.model ? 'Update the noise model' : 'Download the noise model'}
                onClick={() => download.mutate(['model'])}>
                {running && progress?.part === 'model' ? 'Downloading…' : installed?.model ? 'Update' : 'Download'}
              </button>
            ) : <span />}
            {isNoiseModel && installed?.model_problem
              && installed.model_problem !== 'The noise model is not installed' && (
              <small className="catalog-optional-error">{installed.model_problem}</small>
            )}
          </div>
        );
      })}

      {data?.executable && (
        <p className="catalog-footnote">rc-astro on this server: <code>{data.executable}</code></p>
      )}

      {seiza && installed && (
        <div className="catalog-optional" data-part="runtime">
          <div>
            <strong>ONNX Runtime</strong>
            <small>
              {installed.runtime_path
                ?? (seiza.runtime_download
                  ? `For the Seiza noise model · Microsoft’s release ${seiza.runtime_download}, checked against its SHA-256`
                  : 'PSF Guard has no download for this computer. Install ONNX Runtime 1.17 or later and set its path.')}
            </small>
          </div>
          <span className="catalog-optional-state" data-installed={runtimeReady ? 'true' : 'false'}>
            {runtimeReady ? `Installed · ${installed.runtime_version}` : 'Not installed'}
          </span>
          {seiza.runtime_download && !usesOwnRuntime ? (
            <button type="button" className="browse-button" disabled={busy}
              aria-label={runtimeReady ? 'Download ONNX Runtime again' : 'Download ONNX Runtime'}
              onClick={() => download.mutate(['runtime'])}>
              {running && progress?.part === 'runtime' ? 'Downloading…' : 'Download'}
            </button>
          ) : <span />}
          {installed.runtime_problem && installed.runtime_problem !== 'ONNX Runtime is not installed' && (
            <small className="catalog-optional-error">{installed.runtime_problem}</small>
          )}
        </div>
      )}

      {missing.length === 2 && (
        <div className="catalog-actions">
          <button type="button" className="save-button" disabled={busy}
            onClick={() => download.mutate(missing)}>
            {busy ? 'Downloading…' : 'Download both'}
          </button>
        </div>
      )}

      {seiza && installed && canManage && (
        <form className="catalog-install-controls" onSubmit={(event) => {
          event.preventDefault();
          savePath.mutate(shownPath);
        }}>
          <label htmlFor="processing-tools-onnxruntime">ONNX Runtime path</label>
          <input id="processing-tools-onnxruntime" type="text" value={shownPath}
            placeholder={installed.folder ? `Downloaded to ${installed.folder}` : ''}
            onChange={(event) => setPath(event.target.value)} />
          <small>
            Leave empty to use the downloaded ONNX Runtime. To use one you installed, enter its
            library file or the folder that contains it. Restart PSF Guard after you change the
            path of an ONNX Runtime that has already run.
          </small>
          <div className="catalog-actions">
            <button type="submit" className="browse-button"
              disabled={savePath.isPending || shownPath.trim() === savedPath}>
              {savePath.isPending ? 'Saving…' : 'Save path'}
            </button>
          </div>
        </form>
      )}

      {(download.error || savePath.error || stop.error) && (
        <div className="catalog-error" role="alert">
          {(download.error ?? savePath.error ?? stop.error)?.message}
        </div>
      )}

      {progress && (running || progress.finished_at !== undefined) && (
        <div className={`catalog-progress catalog-progress-${running ? 'downloading' : progress.error ? 'error' : 'complete'}`}
          role="status" aria-live="polite">
          <strong>{progress.message}</strong>
          {running && progress.bytes_total !== undefined && progress.bytes_completed !== undefined && (
            <>
              <progress value={progress.bytes_completed} max={progress.bytes_total} />
              <small>{megabytes(progress.bytes_completed)} of {megabytes(progress.bytes_total)}</small>
            </>
          )}
          {running && (
            <div className="catalog-actions">
              <button type="button" className="browse-button" disabled={stop.isPending}
                onClick={() => stop.mutate()}>
                Stop download
              </button>
            </div>
          )}
        </div>
      )}
    </section>
  );
}
