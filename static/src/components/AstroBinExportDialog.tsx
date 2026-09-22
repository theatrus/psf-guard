import { useEffect, useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { AstroBinDetail, AstroBinRow } from '../api/types';
import Dialog from './Dialog';
import './AstroBinExportDialog.css';

/** The target or project whose acquisition rows the dialog shows. */
export interface AstroBinExportRequest {
  dbId: string;
  scope: { project_id?: number; target_id?: number };
  label: string;
}

interface Props {
  request: AstroBinExportRequest;
  onClose: () => void;
}

const DETAIL_HELP: Record<AstroBinDetail, string> = {
  essentials: 'Date, filter, number of frames and exposure length: what every upload needs.',
  full: 'Also binning, gain, sensor and ambient temperature, f-number, and the darks, flats and bias the calibration library matches to each night.',
};

const ASTROBIN_FILTERS_URL = 'https://app.astrobin.com/equipment/explorer/filter';

function formatHours(seconds: number): string {
  const hours = seconds / 3600;
  return hours >= 10 ? `${Math.round(hours)} h` : `${hours.toFixed(1)} h`;
}

/** A number as the CSV writes it: at most `decimals` places, no trailing zeros. */
function cell(value: number | null, decimals = 2): string {
  if (value == null) return '';
  return String(Number(value.toFixed(decimals)));
}

/**
 * The acquisition CSV AstroBin imports on an image's upload page. The
 * dialog previews the rows, lets the person pick how much detail goes in,
 * fills in AstroBin filter ids for any filter that lacks one, and hands
 * the CSV over as a download or through the clipboard.
 */
export default function AstroBinExportDialog({ request, onClose }: Props) {
  const queryClient = useQueryClient();
  const [detail, setDetail] = useState<AstroBinDetail>('essentials');
  // Ungraded lights are what a fresh night mostly is, and the stack previews
  // include them, so the count does too unless told otherwise.
  const [includePending, setIncludePending] = useState(true);
  const [draftIds, setDraftIds] = useState<Record<string, string>>({});
  const [copied, setCopied] = useState(false);

  const params = useMemo(
    () => ({ ...request.scope, include_pending: includePending, detail }),
    [request.scope, includePending, detail]
  );
  const exportQuery = useQuery({
    queryKey: ['astrobin-export', request.dbId, params],
    queryFn: () => apiClient.getAstroBinExport(request.dbId, params),
    staleTime: 60_000,
  });
  const settings = useQuery({
    queryKey: ['astrobin-settings'],
    queryFn: apiClient.getAstroBinSettings,
  });
  const saveIds = useMutation({
    mutationFn: (filterIds: Record<string, number>) =>
      apiClient.updateAstroBinSettings(filterIds),
    onSuccess: (updated) => {
      queryClient.setQueryData(['astrobin-settings'], updated);
      setDraftIds({});
      void queryClient.invalidateQueries({ queryKey: ['astrobin-export', request.dbId] });
    },
  });

  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 2000);
    return () => window.clearTimeout(timer);
  }, [copied]);

  const data = exportQuery.data;
  const unmapped = data?.unmapped_filters ?? [];
  const draftsReady = unmapped.some((filter) => /^\d+$/.test(draftIds[filter]?.trim() ?? ''));
  const saveDrafts = () => {
    const merged: Record<string, number> = { ...(settings.data?.filter_ids ?? {}) };
    for (const filter of unmapped) {
      const id = Number.parseInt(draftIds[filter]?.trim() ?? '', 10);
      if (Number.isFinite(id) && id > 0) merged[filter] = id;
    }
    saveIds.mutate(merged);
  };
  const copyCsv = async () => {
    if (!data) return;
    try {
      await navigator.clipboard.writeText(data.csv);
      setCopied(true);
    } catch {
      alert('Could not reach the clipboard; use Download instead.');
    }
  };

  const full = detail === 'full';

  return (
    <Dialog
      open
      title={`AstroBin acquisitions — ${request.label}`}
      onClose={onClose}
      className="astrobin-dialog"
      footer={
        <>
          <button type="button" className="header-button" onClick={onClose}>
            Close
          </button>
          <button
            type="button"
            className="header-button"
            disabled={!data || data.rows.length === 0}
            onClick={copyCsv}
          >
            {copied ? 'Copied' : 'Copy CSV'}
          </button>
          <a
            className={`action-button astrobin-download${
              !data || data.rows.length === 0 ? ' is-disabled' : ''
            }`}
            href={apiClient.astroBinCsvUrl(request.dbId, params)}
            download={data?.filename}
            aria-disabled={!data || data.rows.length === 0}
          >
            Download CSV
          </a>
        </>
      }
    >
      <p className="astrobin-scope">
        One row per night, filter and exposure length, ready for the CSV import on an
        AstroBin upload&apos;s acquisition step. Rejected lights never count.
      </p>

      <fieldset className="export-layout-options">
        <legend>Detail</legend>
        {(['essentials', 'full'] as const).map((option) => (
          <label key={option} className="export-layout-option">
            <input
              type="radio"
              name="astrobin-detail"
              value={option}
              checked={detail === option}
              onChange={() => setDetail(option)}
            />
            <span>
              <strong>{option === 'full' ? 'Full' : 'Essentials'}</strong>
              <small>{DETAIL_HELP[option]}</small>
            </span>
          </label>
        ))}
      </fieldset>

      <label className="export-dialog-option astrobin-pending">
        <input
          type="checkbox"
          checked={includePending}
          onChange={(event) => setIncludePending(event.target.checked)}
        />
        <span>
          <strong>Count ungraded lights</strong>
          <small>Frames the grader has not judged yet, as the stack previews do.</small>
        </span>
      </label>

      {exportQuery.isLoading && <p className="astrobin-muted">Counting frames…</p>}
      {exportQuery.isError && (
        <p className="astrobin-error">{(exportQuery.error as Error).message}</p>
      )}

      {data && (
        <>
          <p className="astrobin-summary">
            <strong>{data.frames}</strong> frame{data.frames === 1 ? '' : 's'} over{' '}
            <strong>{data.nights}</strong> night{data.nights === 1 ? '' : 's'},{' '}
            <strong>{formatHours(data.total_exposure_seconds)}</strong> of exposure.
            {data.lights_missing_files > 0 &&
              ` ${data.lights_missing_files} light file(s) were not found; their nights borrow another light's header where one exists.`}
          </p>
          {data.notes.map((note) => (
            <p key={note} className="astrobin-muted">
              {note}
            </p>
          ))}

          {unmapped.length > 0 && (
            <div className="astrobin-unmapped" role="group" aria-label="Filters without an AstroBin id">
              <p>
                AstroBin knows filters by the number in the address of their page in its{' '}
                <a href={ASTROBIN_FILTERS_URL} target="_blank" rel="noreferrer">
                  equipment database
                </a>
                . Rows for these filters have an empty filter cell until you enter one:
              </p>
              <div className="astrobin-unmapped-grid">
                {unmapped.map((filter) => (
                  <label key={filter} className="astrobin-unmapped-row">
                    <span>{filter}</span>
                    <input
                      type="text"
                      inputMode="numeric"
                      placeholder="AstroBin filter id"
                      aria-label={`AstroBin id for filter ${filter}`}
                      value={draftIds[filter] ?? ''}
                      onChange={(event) =>
                        setDraftIds((prev) => ({ ...prev, [filter]: event.target.value }))
                      }
                    />
                  </label>
                ))}
              </div>
              <button
                type="button"
                className="header-button"
                disabled={!draftsReady || saveIds.isPending}
                onClick={saveDrafts}
              >
                Save filter ids
              </button>
              {saveIds.isError && (
                <p className="astrobin-error">{(saveIds.error as Error).message}</p>
              )}
            </div>
          )}

          {data.rows.length === 0 ? (
            <p className="astrobin-muted">No lights to count.</p>
          ) : (
            <div className="astrobin-table-wrap">
              <table className="astrobin-table">
                <thead>
                  <tr>
                    <th>Date</th>
                    <th>Filter</th>
                    <th>Frames</th>
                    <th>Seconds</th>
                    {full && (
                      <>
                        <th>Bin</th>
                        <th>Gain</th>
                        <th title="Mean sensor temperature, °C">Sensor</th>
                        <th>f/</th>
                        <th>Darks</th>
                        <th>Flats</th>
                        <th title="Dark flats">Flat darks</th>
                        <th>Bias</th>
                        <th title="Mean focuser probe temperature, °C">Ambient</th>
                      </>
                    )}
                  </tr>
                </thead>
                <tbody>
                  {data.rows.map((row: AstroBinRow) => (
                    <tr key={`${row.date}-${row.filter}-${row.duration}-${row.binning}-${row.gain}`}>
                      <td>{row.date}</td>
                      <td className={row.filter_id == null ? 'astrobin-unmapped-cell' : ''}>
                        {row.filter}
                        {row.filter_id != null && <small> #{row.filter_id}</small>}
                      </td>
                      <td>{row.number}</td>
                      <td>{cell(row.duration, 4)}</td>
                      {full && (
                        <>
                          <td>{cell(row.binning)}</td>
                          <td>{cell(row.gain)}</td>
                          <td>{cell(row.sensor_cooling)}</td>
                          <td>{cell(row.f_number)}</td>
                          <td>{cell(row.darks)}</td>
                          <td>{cell(row.flats)}</td>
                          <td>{cell(row.flat_darks)}</td>
                          <td>{cell(row.bias)}</td>
                          <td>{cell(row.temperature)}</td>
                        </>
                      )}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </>
      )}
    </Dialog>
  );
}
