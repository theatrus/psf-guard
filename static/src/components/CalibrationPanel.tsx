import { useEffect, useRef, useState, type ReactNode } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Link, useLocation } from 'react-router-dom';
import { apiClient } from '../api/client';
import type {
  CalibrationKind,
  CalibrationKindExplanation,
  CalibrationNightFrames,
  ImageCalibrationExplanation,
  LightCalibrationFinding,
} from '../api/types';
import { CALIBRATION_FOCUS_PARAM, useFilters, useUrlParams } from '../hooks/useUrlState';
import { calibrationPageHref } from './calibration/calibrationLinks';
import { CalibrationIcon } from './ScoreChipIcons';
import {
  FINDING_HINTS,
  NO_FINDING,
  calibrationKindLabel,
  chosenFinding,
  focusParts,
  nightsApart,
  temperatureRange,
  type CalibrationPart,
} from '../utils/calibrationFocus';
import './CalibrationPanel.css';

interface CalibrationPanelProps {
  dbId: string;
  imageId: number;
  projectId: number | null;
}

/** Refusals shown before "N more". */
const REFUSED_SHOWN = 4;

function plural(count: number, one: string, many = `${one}s`): string {
  return `${count} ${count === 1 ? one : many}`;
}

/** The readings the library is matched on, as the light's header gives them. */
function lightReadings(light: ImageCalibrationExplanation['light']): string {
  const parts = [
    light.filter,
    light.exposure_s != null ? `${light.exposure_s} s` : null,
    light.gain != null ? `gain ${light.gain}` : null,
    light.offset != null ? `offset ${light.offset}` : null,
    light.binning ? `bin ${light.binning}` : null,
    temperatureRange(light.camera_temp, light.camera_temp),
  ];
  return parts.filter(Boolean).join(' · ');
}

function hasAnything(kind: CalibrationKindExplanation): boolean {
  return kind.master.frames > 0
    || kind.matching_unused.length > 0
    || kind.refused.length > 0
    || kind.kept_out.length > 0
    || kind.other_filters > 0
    || kind.other_cameras > 0;
}

/**
 * What the calibration library gives one light, kind by kind, and what the
 * health check finds wrong with it. Choosing a finding (kept in the URL, so
 * it stays chosen from image to image) picks out the frames behind it and
 * folds the kinds it is not about.
 */
export default function CalibrationPanel({ dbId, imageId, projectId }: CalibrationPanelProps) {
  const { getParam, updateParams } = useUrlParams();
  const { filters } = useFilters();
  const location = useLocation();
  const focus = getParam(CALIBRATION_FOCUS_PARAM);
  const sectionRef = useRef<HTMLElement>(null);
  const scrolledFor = useRef<string | null>(null);

  const query = useQuery({
    queryKey: ['db', dbId, 'image-calibration', imageId],
    queryFn: () => apiClient.getImageCalibration(dbId, imageId),
    staleTime: 60_000,
    retry: false,
  });

  // Opened on purpose, from a chip, a chosen finding or Images filtered by
  // calibration: bring the panel into view once per image and choice, so
  // later refreshes leave the scroll alone.
  const purpose = focus ?? filters.calibration.join(',');
  useEffect(() => {
    const key = `${imageId}:${purpose}`;
    if (!purpose || !query.data || scrolledFor.current === key) return;
    scrolledFor.current = key;
    sectionRef.current?.scrollIntoView?.({ block: 'nearest' });
  }, [purpose, imageId, query.data]);

  const heading = (badge?: ReactNode) => (
    <div className="light-calibration-panel-heading">
      <h3>
        <CalibrationIcon /> Calibration
      </h3>
      {badge}
    </div>
  );

  if (query.isLoading) {
    return (
      <section className="info-section light-calibration-panel" ref={sectionRef} data-testid="calibration-panel">
        {heading()}
        <p className="light-calibration-panel-message">Matching the light against the library…</p>
      </section>
    );
  }
  if (query.error || !query.data) {
    return (
      <section className="info-section light-calibration-panel" ref={sectionRef} data-testid="calibration-panel">
        {heading()}
        <p className="light-calibration-panel-message">
          {query.error instanceof Error ? query.error.message : 'Calibration information is unavailable.'}
        </p>
      </section>
    );
  }

  const explanation = query.data;
  const { findings } = explanation;
  const chosen = chosenFinding(findings, focus, filters.calibration);
  const worst = findings[0]?.level ?? null;
  const choose = (finding: LightCalibrationFinding) =>
    updateParams({ [CALIBRATION_FOCUS_PARAM]: chosen?.code === finding.code ? NO_FINDING : finding.code });
  const kinds = explanation.kinds.filter(hasAnything);

  const calibrationHref = (kind: CalibrationKind, night?: string) => {
    const [path, search] = calibrationPageHref(location.search, dbId, projectId ?? undefined).split('?');
    const params = new URLSearchParams(search);
    params.set('calKind', kind);
    if (kind === 'flat' && explanation.light.filter) params.set('calFilter', explanation.light.filter);
    if (night) {
      params.set('calFrom', night);
      params.set('calTo', night);
    }
    return `${path}?${params}`;
  };

  return (
    <section
      className={`info-section light-calibration-panel${worst ? ` is-${worst}` : ''}`}
      ref={sectionRef}
      data-testid="calibration-panel"
    >
      {heading(
        <span className={`light-calibration-panel-badge${worst ? ` is-${worst}` : ' is-ok'}`}>
          {worst === 'poor' ? 'Poor' : worst === 'check' ? 'Check' : 'No findings'}
        </span>
      )}
      {lightReadings(explanation.light) && (
        <p className="light-calibration-panel-readings" title="The readings the library is matched on">
          Matched on {lightReadings(explanation.light)}
        </p>
      )}

      {findings.length > 0 ? (
        <>
          <ul className="light-calibration-findings" aria-label="Calibration findings">
            {findings.map((finding) => {
              const isChosen = chosen?.code === finding.code;
              return (
                <li key={finding.code}>
                  <button
                    type="button"
                    className={`light-calibration-finding is-${finding.level}`}
                    aria-pressed={isChosen}
                    onClick={() => choose(finding)}
                  >
                    <span className="light-calibration-finding-level">
                      {finding.level === 'poor' ? 'Poor' : 'Check'}
                    </span>
                    <span className="light-calibration-finding-reason">{finding.reason}</span>
                  </button>
                </li>
              );
            })}
          </ul>
          {chosen ? (
            <div className="light-calibration-focus-note">
              <p>{FINDING_HINTS[chosen.code]}</p>
              <button
                type="button"
                className="light-calibration-show-all"
                onClick={() => updateParams({ [CALIBRATION_FOCUS_PARAM]: NO_FINDING })}
              >
                Show every kind
              </button>
            </div>
          ) : (
            <p className="light-calibration-panel-message">Choose a finding to pick out the frames behind it.</p>
          )}
        </>
      ) : (
        <p className="light-calibration-panel-message">The library calibrates this light with no findings.</p>
      )}

      {kinds.length === 0 && (
        <p className="light-calibration-panel-message">The library holds no calibration frames.</p>
      )}
      {kinds.map((kind) => {
        const parts = focusParts(chosen, kind);
        return (
          <KindAccount
            key={kind.kind}
            kind={kind}
            parts={parts}
            folded={chosen !== null && parts.size === 0}
            level={chosen?.level ?? null}
            lightNight={explanation.light.night}
            hrefFor={(night) => calibrationHref(kind.kind, night)}
          />
        );
      })}
    </section>
  );
}

interface KindAccountProps {
  kind: CalibrationKindExplanation;
  parts: ReadonlySet<CalibrationPart>;
  /** About something else: one line only. */
  folded: boolean;
  level: LightCalibrationFinding['level'] | null;
  lightNight: string | null;
  hrefFor: (night?: string) => string;
}

function KindAccount({ kind, parts, folded, level, lightNight, hrefFor }: KindAccountProps) {
  const [allRefused, setAllRefused] = useState(false);
  const label = calibrationKindLabel(kind.kind);
  const focused = (part: CalibrationPart) => (parts.has(part) ? ` is-focus is-${level}` : '');
  const { master } = kind;
  const summary = master.frames > 0
    ? [
        plural(master.frames, 'frame'),
        plural(master.nights, 'night'),
        temperatureRange(master.temp_min_c, master.temp_max_c),
      ].filter(Boolean).join(' · ')
    : 'None matches';

  const nightRow = (night: CalibrationNightFrames, part: CalibrationPart | null) => {
    const apart = kind.kind === 'flat' ? nightsApart(night.night, lightNight) : null;
    const note = night.night === lightNight
      ? "the light's night"
      : apart != null && apart > 0
        ? `${plural(apart, 'day')} from the light`
        : null;
    return (
      <li key={night.night} className={`light-calibration-night${part ? focused(part) : ''}`}>
        <Link to={hrefFor(night.night)} title={`Open these ${label.toLowerCase()} in Calibration`}>
          {night.night}
        </Link>
        <span>{plural(night.frames, 'frame')}</span>
        {temperatureRange(night.temp_min_c, night.temp_max_c) && (
          <span className="light-calibration-muted">{temperatureRange(night.temp_min_c, night.temp_max_c)}</span>
        )}
        {note && <span className="light-calibration-night-note">{note}</span>}
      </li>
    );
  };

  const refused = allRefused || parts.has('refused') ? kind.refused : kind.refused.slice(0, REFUSED_SHOWN);

  return (
    <div
      className={`light-calibration-kind${folded ? ' is-folded' : ''}${parts.size > 0 ? ` is-about is-${level}` : ''}`}
      data-testid={`calibration-kind-${kind.kind}`}
    >
      <div className={`light-calibration-kind-heading${focused('master')}`}>
        <strong>{label}</strong>
        <span className={master.frames > 0 ? '' : 'light-calibration-missing'}>{summary}</span>
        <Link className="light-calibration-kind-link" to={hrefFor()} title={`Open the ${label.toLowerCase()} in Calibration`}>
          Open
        </Link>
      </div>
      {!folded && (
        <div className="light-calibration-kind-body">
          {kind.used.length > 0 && (
            <div className="light-calibration-part">
              <span className="light-calibration-part-label">In the master</span>
              <ul>{kind.used.map((night) => nightRow(night, 'used'))}</ul>
            </div>
          )}
          {kind.suspect.length > 0 && (
            <div className={`light-calibration-part${focused('suspect')}`}>
              <span className="light-calibration-part-label">Suspect frames it takes</span>
              <ul>
                {kind.suspect.map((frame) => (
                  <li key={`${frame.night}:${frame.file}`} className="light-calibration-frame">
                    <span className="light-calibration-file">{frame.file}</span>
                    <span className="light-calibration-muted">{frame.night}</span>
                    <span className="light-calibration-frame-reason">{frame.reason}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
          {kind.kept_out.length > 0 && (
            <div className={`light-calibration-part${focused('kept_out')}`}>
              <span className="light-calibration-part-label">Kept out</span>
              <ul>
                {kind.kept_out.map((frame) => (
                  <li key={`${frame.night}:${frame.file}`} className="light-calibration-frame">
                    <span className="light-calibration-file">{frame.file}</span>
                    <span className="light-calibration-muted">{frame.night}</span>
                    <span className="light-calibration-frame-reason">{frame.reason}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
          {kind.matching_unused.length > 0 && (
            <div className="light-calibration-part">
              <span className="light-calibration-part-label">Also match, not used</span>
              <ul>{kind.matching_unused.map((night) => nightRow(night, null))}</ul>
            </div>
          )}
          {kind.refused.length > 0 && (
            <div className={`light-calibration-part${focused('refused')}`}>
              <span className="light-calibration-part-label">Refused</span>
              <ul>
                {refused.map((entry) => (
                  <li key={`${entry.night}:${entry.reason}`} className="light-calibration-frame">
                    <span className="light-calibration-muted">{entry.night}</span>
                    <span>{plural(entry.frames, 'frame')}</span>
                    <span className="light-calibration-frame-reason">{entry.reason}</span>
                  </li>
                ))}
              </ul>
              {refused.length < kind.refused.length && (
                <button type="button" className="light-calibration-more" onClick={() => setAllRefused(true)}>
                  {kind.refused.length - refused.length} more
                </button>
              )}
            </div>
          )}
          {(kind.other_filters > 0 || kind.other_cameras > 0) && (
            <p className={`light-calibration-part light-calibration-other${focused('other')}`}>
              {[
                kind.other_filters > 0 ? `${plural(kind.other_filters, 'frame')} for other filters` : null,
                kind.other_cameras > 0 ? `${plural(kind.other_cameras, 'frame')} from other cameras` : null,
              ].filter(Boolean).join(' · ')}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
