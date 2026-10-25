import { useState } from 'react';
import type { StackColorCombination, StackColorRole, StackEmissionProcessing } from '../api/types';
const labels: Partial<Record<StackColorRole, string>> = {
  ha: 'Hα',
  oiii: 'OIII',
  sii: 'SII',
  red: 'R',
  green: 'G',
  blue: 'B',
};

export default function StackEmissionControls({
  value,
  combination,
  disabled,
  onChange,
  view = 'fit',
}: {
  view?: 'fit' | 'blend';
  value: StackEmissionProcessing;
  combination: StackColorCombination;
  disabled: boolean;
  onChange: (value: StackEmissionProcessing) => void;
}) {
  const [fluxError, setFluxError] = useState<string | null>(null);
  const updateLine = (
    role: StackColorRole,
    patch: Partial<NonNullable<StackEmissionProcessing['lines'][StackColorRole]>>,
  ) => {
    const line = value.lines[role];
    if (line) onChange({ ...value, lines: { ...value.lines, [role]: { ...line, ...patch } } });
  };
  if (view === 'blend') {
    return (
      <section
        className="stack-emission-controls"
        aria-label={combination === 'emission' ? 'Emission blend' : 'Broadband structure blend'}
      >
        <header>
          <strong>{combination === 'emission' ? 'Emission blend' : 'Broadband structure'}</strong>
          <span>
            {combination === 'emission' ? 'After color calibration' : 'Before restoring stars'}
          </span>
        </header>
        {combination === 'emission' ? (
          <>
            <div className="stack-emission-lines">
              {Object.entries(value.lines).map(
                ([key, line]) =>
                  line && (
                    <label className="stack-stretch-field" key={key}>
                      <span>{labels[key as StackColorRole]} emission amount</span>
                      <input
                        type="number"
                        aria-label={`${labels[key as StackColorRole]} emission amount`}
                        min="0"
                        max="5"
                        step="0.05"
                        value={line.amount}
                        disabled={disabled}
                        onChange={(event) =>
                          updateLine(key as StackColorRole, { amount: event.target.valueAsNumber })
                        }
                      />
                    </label>
                  ),
              )}
            </div>
            <label className="stack-emission-check">
              <input
                type="checkbox"
                aria-label="Add positive emission only"
                checked={value.positive_contribution}
                disabled={disabled}
                onChange={(event) =>
                  onChange({ ...value, positive_contribution: event.target.checked })
                }
              />
              Add positive emission only; signed residual files retain negative pixels.
            </label>
          </>
        ) : (
          <label className="stack-stretch-field">
            <span>Broadband structure amount</span>
            <input
              type="number"
              aria-label="Broadband structure amount"
              min="0"
              max="1"
              step="0.05"
              value={value.broadband_amount}
              disabled={disabled}
              onChange={(event) =>
                onChange({ ...value, broadband_amount: event.target.valueAsNumber })
              }
            />
            <small>
              0 keeps mapped nebula; 1 uses the prepared broadband nebula. RGB stars are restored
              once.
            </small>
          </label>
        )}
      </section>
    );
  }
  return (
    <section className="stack-emission-controls" aria-label="Continuum subtraction">
      <header>
        <strong>Continuum subtraction</strong>
        <span>Before star removal</span>
      </header>
      <label className="stack-stretch-field">
        Continuum mode{' '}
        <select
          aria-label="Continuum mode"
          disabled={disabled}
          value={value.mode}
          onChange={(event) =>
            onChange({ ...value, mode: event.target.value as StackEmissionProcessing['mode'] })
          }
        >
          <option value="automatic">Automatic · validate stellar fit</option>
          <option value="manual">Manual weights</option>
          <option value="off">Off</option>
        </select>
      </label>
      <p className="processing-setups-hint">
        Fit aligned linear images before star removal. Producer clipping maps exclude saturated
        stars. Missing evidence or an unreliable fit skips that line in RGB/LRGB; a mapped palette
        stops and keeps the previous preview.
      </p>
      {value.mode === 'manual' && (
        <div className="stack-emission-lines">
          {Object.entries(value.lines).map(([key, line]) => {
            const role = key as StackColorRole;
            if (!line) return null;
            const references = role === 'oiii' ? ['G', 'B'] : ['R'];
            return (
              <fieldset className="stack-emission-line" key={role} disabled={disabled}>
                <legend>
                  {labels[role]} − {references.join(' / ')} continuum
                </legend>
                {value.mode === 'manual' &&
                  references.map((reference, index) => (
                    <label className="stack-stretch-field" key={reference}>
                      {reference} weight{' '}
                      <input
                        type="number"
                        min="0"
                        step="0.001"
                        aria-label={`${labels[role]} ${reference} continuum weight`}
                        value={line.coefficients[index] ?? ''}
                        onChange={(event) => {
                          const coefficients = [...line.coefficients];
                          coefficients[index] = event.target.valueAsNumber;
                          updateLine(role, { coefficients });
                        }}
                      />
                    </label>
                  ))}
                {value.mode === 'manual' && role === 'oiii' && (
                  <label className="stack-stretch-field">
                    Stellar B/G flux ratio{' '}
                    <input
                      type="number"
                      min="0.000001"
                      step="0.05"
                      aria-label="OIII stellar B/G flux ratio"
                      value={line.reference_flux_ratios[1] ?? ''}
                      onChange={(event) =>
                        updateLine(role, {
                          reference_flux_ratios:
                            event.target.value === '' ? [] : [1, event.target.valueAsNumber],
                        })
                      }
                    />
                    <small>
                      Leave blank to use the measured ratio. Enter a ratio if automatic photometry
                      cannot measure one.
                    </small>
                  </label>
                )}
              </fieldset>
            );
          })}
        </div>
      )}
      <details className="stack-processing-advanced">
        <summary>Advanced fit and saturation limits</summary>
        <label className="stack-emission-check">
          <input
            type="checkbox"
            aria-label="Match continuum PSFs"
            disabled={disabled || (value.mode !== 'automatic' && !value.photometry?.psfMatching)}
            checked={!!value.photometry?.psfMatching}
            onChange={(event) =>
              onChange({
                ...value,
                photometry: { ...value.photometry, psfMatching: event.target.checked ? {} : null },
              })
            }
          />
          Match continuum PSFs with Gaussian broadening
        </label>
        <p className="processing-setups-hint">
          Broadens copies used for fitting and subtraction. RGB stars keep their own processing.
          Held-out pixel profiles must still pass; Gaussian matching may leave different halo
          shapes.
        </p>
        {value.photometry?.psfMatching && (
          <label className="stack-stretch-field">
            Target FWHM (pixels){' '}
            <input
              type="number"
              min="0.1"
              max={Math.min(
                32,
                value.photometry?.apertureRadius ?? 8,
                (value.photometry?.annulusInner ?? 12) / 1.5,
              )}
              step="0.1"
              aria-label="Continuum target FWHM"
              disabled={disabled}
              value={value.photometry.psfMatching.targetFwhm ?? ''}
              onChange={(event) =>
                onChange({
                  ...value,
                  photometry: {
                    ...value.photometry,
                    psfMatching: {
                      targetFwhm: event.target.value === '' ? null : event.target.valueAsNumber,
                    },
                  },
                })
              }
            />
            <small>
              Blank chooses 1.05 times the broadest measured profile. The target must fit inside the
              photometry aperture and below the sky annulus. A target below any measured width is
              refused.
            </small>
          </label>
        )}
        <label className="stack-stretch-field">
          Usable star budget{' '}
          <input
            type="number"
            aria-label="Continuum usable star budget"
            min="12"
            max="4096"
            value={value.fit?.maxStars ?? 400}
            disabled={disabled}
            onChange={(event) =>
              onChange({ ...value, fit: { ...value.fit, maxStars: event.target.valueAsNumber } })
            }
          />
        </label>
        <p className="processing-setups-hint">
          Leave working-unit saturation limits blank to use producer clipping maps. A limit must
          describe these exact prepared pixels; an image maximum is not a saturation limit.
        </p>
        <div className="stack-stretch-fields">
          {[
            ...new Set([
              ...Object.keys(value.lines),
              'red',
              ...(value.lines.oiii ? ['green', 'blue'] : []),
            ]),
          ].map((key) => {
            const role = key as StackColorRole;
            return (
              <label className="stack-stretch-field" key={role}>
                {labels[role]} saturation{' '}
                <input
                  type="number"
                  aria-label={`${labels[role]} continuum saturation`}
                  min="0"
                  value={value.saturation?.[role] ?? ''}
                  disabled={disabled}
                  onChange={(event) => {
                    const saturation = { ...value.saturation };
                    if (event.target.value === '') delete saturation[role];
                    else saturation[role] = event.target.valueAsNumber;
                    onChange({ ...value, saturation });
                  }}
                />
              </label>
            );
          })}
        </div>
      </details>
      <details className="stack-processing-advanced">
        <summary>Calibrated line-flux export</summary>
        <p className="processing-setups-hint">
          Optional scientific FITS export. Supply calibrated NB and reference responses in the exact
          source sample units, explicit calibrated residual zero levels, a shared flux unit and
          calibration revision. The server subtracts broadband line leakage and solves coupled
          responses. Filter names and widths cannot supply this calibration. Display amounts still
          use the signed residuals.
        </p>
        <label className="stack-stretch-field">
          Response calibration JSON{' '}
          <input
            type="file"
            accept="application/json,.json"
            aria-label="Response calibration JSON"
            disabled={disabled || value.mode === 'off'}
            onChange={async (event) => {
              const file = event.target.files?.[0];
              if (!file) return;
              try {
                const parsed: unknown = JSON.parse(await file.text());
                if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed))
                  throw new Error('Response calibration must be a JSON object.');
                onChange({ ...value, flux_calibration: parsed as Record<string, unknown> });
                setFluxError(null);
              } catch (error) {
                setFluxError(
                  error instanceof Error
                    ? error.message
                    : 'The calibration file could not be read.',
                );
              }
            }}
          />
        </label>
        {value.flux_calibration && (
          <p>
            Calibration loaded: {String(value.flux_calibration.revision ?? 'revision missing')} ·{' '}
            {String(value.flux_calibration.unit ?? 'unit missing')}{' '}
            <button
              type="button"
              disabled={disabled}
              onClick={() => onChange({ ...value, flux_calibration: null })}
            >
              Remove flux calibration
            </button>
          </p>
        )}
        {fluxError && <p role="alert">{fluxError}</p>}
        <small>
          Use the exact source objects from each full stellar fit report. Source or fitted-weight
          changes require a new calibration with exact continuum coefficient snapshots. Missing or
          unstable line responses stop the build. The server never infers zero levels from the
          display baseline. Unknown covariance is reported as unknown uncertainty.
        </small>
      </details>
    </section>
  );
}
