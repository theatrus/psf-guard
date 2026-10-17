import { useEffect, useRef, useState, type ReactNode } from 'react';
import { useQuery } from '@tanstack/react-query';
import { Link, useLocation } from 'react-router-dom';
import { apiClient } from '../api/client';
import type {
  CalibrationHealthCode,
  CalibrationHealthLevel,
  CalibrationKind,
  CalibrationKindExplanation,
  CalibrationNightFrames,
  FlatFamilyNote,
  ImageCalibrationExplanation,
  LightCalibrationFinding,
} from '../api/types';
import { CALIBRATION_FOCUS_PARAM, useFilters, useUrlParams } from '../hooks/useUrlState';
import { useCalibrationMarks } from '../hooks/useCalibrationMarks';
import { useCalibrationSection } from '../hooks/useDetailSections';
import { calibrationFilterLabel } from '../utils/calibrationFilter';
import { openSettings } from '../utils/settingsIntent';
import { useAccess } from '../auth/access';
import { calibrationPageHref } from './calibration/calibrationLinks';
import DetailSection from './DetailSection';
import FlatChoicePicker from './FlatChoicePicker';
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
import { useNightSplit } from '../hooks/useNightSplit';

interface CalibrationPanelProps {
  dbId: string;
  imageId: number;
  projectId: number | null;
}

/** Bases that leave a light's flats from across a change: worth a check. */
const UNSETTLED_BASES = new Set(['no_same_side', 'undetermined']);

/** How a light's flat family was chosen, in a few words. */
function familyBasisLabel(family: FlatFamilyNote): string {
  switch (family.by) {
    case 'person':
      return 'chosen by you';
    case 'pixels':
      return family.moved
        ? `moved by its own pixels, confirmed by ${family.corroborated ?? 'other evidence'}`
        : 'chosen by its own pixels';
    case 'nearest':
      return family.undecided
        ? 'at its recorded angle: its lights could not tell'
        : 'at its recorded angle: its lights not read yet';
    case 'disputed':
      return `check: its lights point to ${family.pointed ?? 'another family'}, unconfirmed`;
    default:
      return 'its angles measure alike';
  }
}

/** A fraction as a percent with one decimal: `0.2%`. */
/** A fraction in percent: two decimals below 1%, and `<0.01%` below that. */
function fractionPercent(value: number): string {
  const percent = value * 100;
  if (percent < 0.01) return '<0.01%';
  return `${percent >= 1 ? percent.toFixed(1) : percent.toFixed(2)}%`;
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
 *
 * The panel folds away; `useCalibrationSection` says when it starts open.
 * Folded, its heading says what the project's calibration marks say about
 * the light, and it asks the server for nothing more: the light's own
 * account is fetched when the panel opens.
 */
export default function CalibrationPanel({ dbId, imageId, projectId }: CalibrationPanelProps) {
  const { getParam, updateParams } = useUrlParams();
  const { filters } = useFilters();
  const location = useLocation();
  const focus = getParam(CALIBRATION_FOCUS_PARAM);
  const sectionRef = useRef<HTMLElement>(null);
  const scrolledFor = useRef<string | null>(null);

  // The project's marks: cached, and shared with the chips in Images.
  const calibrationMarks = useCalibrationMarks(dbId, projectId, true);
  const mark = calibrationMarks.marks.get(imageId);
  // Opened on purpose, from a chip, a chosen finding or Images filtered by
  // calibration.
  const purpose = focus ?? filters.calibration.join(',');
  const section = useCalibrationSection(!!mark, purpose);

  // The light's night as the view names it (useNightSplit).
  const nightSplit = useNightSplit();
  const query = useQuery({
    queryKey: ['db', dbId, 'image-calibration', imageId, nightSplit.queryKey],
    queryFn: () => apiClient.getImageCalibration(dbId, imageId, nightSplit.override),
    staleTime: 60_000,
    retry: false,
    enabled: section.open,
  });

  // Opened on purpose: bring the panel into view once per image and
  // choice, so later refreshes leave the scroll alone.
  useEffect(() => {
    const key = `${imageId}:${purpose}`;
    if (!section.open || !purpose || !query.data || scrolledFor.current === key) return;
    scrolledFor.current = key;
    sectionRef.current?.scrollIntoView?.({ block: 'nearest' });
  }, [purpose, imageId, query.data, section.open]);

  // The light's own account, unless asking for it failed.
  const explanation = query.error ? undefined : query.data;
  const findings = explanation?.findings ?? [];
  const chosen = chosenFinding(findings, focus, filters.calibration);
  // The heading reads the light's own account once it is here, else its mark.
  const worst: CalibrationHealthLevel | null = explanation
    ? findings[0]?.level ?? null
    : mark?.level ?? null;
  const codes: CalibrationHealthCode[] = explanation
    ? findings.map((finding) => finding.code)
    : mark?.codes ?? [];
  const summary = codes.length > 0
    ? codes.map(calibrationFilterLabel).join(' · ')
    : explanation
      ? 'no findings'
      : calibrationMarks.isLoading
        ? undefined
        : calibrationMarks.updating
          ? 'being checked'
          : 'no findings';
  const choose = (finding: LightCalibrationFinding) =>
    updateParams({ [CALIBRATION_FOCUS_PARAM]: chosen?.code === finding.code ? NO_FINDING : finding.code });
  const kinds = explanation?.kinds.filter(hasAnything) ?? [];

  const calibrationHref = (kind: CalibrationKind, night?: string) => {
    // The panel accounts for every frame the light matched, refused or
    // lacks, most of which its project's scope leaves out, so its links
    // open the rig's whole library narrowed to the kind and night.
    const [path, search] = calibrationPageHref(location.search, dbId, projectId ?? undefined, { scoped: false }).split('?');
    const params = new URLSearchParams(search);
    params.set('calKind', kind);
    if (kind === 'flat' && explanation?.light.filter) params.set('calFilter', explanation.light.filter);
    if (night) {
      params.set('calFrom', night);
      params.set('calTo', night);
    }
    return `${path}?${params}`;
  };

  return (
    <DetailSection
      ref={sectionRef}
      className={`light-calibration-panel${worst ? ` is-${worst}` : ''}`}
      headingClassName="light-calibration-panel-heading"
      data-testid="calibration-panel"
      icon={<CalibrationIcon />}
      title="Calibration"
      badge={(worst || explanation) && (
        <span className={`light-calibration-panel-badge${worst ? ` is-${worst}` : ' is-ok'}`}>
          {worst === 'poor' ? 'Poor' : worst === 'check' ? 'Check' : 'No findings'}
        </span>
      )}
      summary={summary}
      open={section.open}
      onToggle={section.toggle}
    >
      {query.isPending && (
        <p className="light-calibration-panel-message">Matching the light against the library…</p>
      )}
      {!query.isPending && !explanation && (
        <p className="light-calibration-panel-message">
          {query.error instanceof Error ? query.error.message : 'Calibration information is unavailable.'}
        </p>
      )}
      {explanation && lightReadings(explanation.light) && (
        <p className="light-calibration-panel-readings" title="The readings the library is matched on">
          Matched on {lightReadings(explanation.light)}
        </p>
      )}

      {explanation && (findings.length > 0 ? (
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
            <p className="light-calibration-panel-message">Choose a finding to show the affected frames.</p>
          )}
        </>
      ) : (
        <p className="light-calibration-panel-message">The library calibrates this light with no findings.</p>
      ))}

      {explanation && <LightRules explanation={explanation} />}

      {explanation && kinds.length === 0 && (
        <p className="light-calibration-panel-message">The library contains no calibration frames.</p>
      )}
      {explanation && kinds.map((kind) => {
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
            extra={kind.kind === 'flat' ? <FlatChoicePicker dbId={dbId} imageId={imageId} kind={kind} rig={explanation.rules?.rig_uuid} /> : null}
          />
        );
      })}
    </DetailSection>
  );
}

const SOURCE_LABEL: Record<string, string> = {
  global: 'global',
  rig: 'this rig',
  default: 'default',
};

/**
 * The rules the light was checked by: what a setting kept from being
 * flagged, and every limit that differs from its default with who set it.
 * An editor can open the rig's own settings from here.
 */
function LightRules({ explanation }: { explanation: ImageCalibrationExplanation }) {
  const { canWrite } = useAccess();
  const rules = explanation.rules;
  const waived = explanation.waived ?? [];
  if (!rules && waived.length === 0) return null;
  const notes = rules?.notes ?? [];
  return (
    <div className="light-calibration-rules" data-testid="calibration-rules">
      {waived.length > 0 && (
        <div className="light-calibration-part">
          <span className="light-calibration-part-label">Not flagged</span>
          <ul className="light-calibration-waived">
            {waived.map((finding) => (
              <li key={`${finding.code}:${finding.reason}`}>{finding.reason}</li>
            ))}
          </ul>
        </div>
      )}
      {rules && (
        <div className="light-calibration-part">
          <span className="light-calibration-part-label">
            {rules.rig_has_settings ? `Checked by ${rules.rig_name}'s own settings` : 'Checked by the global settings'}
          </span>
          {notes.length > 0 ? (
            <ul className="light-calibration-rule-notes" aria-label="Settings that differ from the defaults">
              {notes.map((note) => (
                <li key={note.key}>
                  <span>{note.label}</span>
                  <strong>{note.text}</strong>
                  <span className="light-calibration-muted">{SOURCE_LABEL[note.source] ?? note.source}</span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="light-calibration-muted">Every limit is at its default.</p>
          )}
          {canWrite && (
            <button
              type="button"
              className="light-calibration-tune"
              onClick={() =>
                openSettings('calibration', { rig: { uuid: rules.rig_uuid, name: rules.rig_name } })
              }
            >
              Tune checks for {rules.rig_name}
            </button>
          )}
        </div>
      )}
    </div>
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
  /** What a person can choose for these frames, below how they were chosen. */
  extra?: ReactNode;
}

function KindAccount({ kind, parts, folded, level, lightNight, hrefFor, extra }: KindAccountProps) {
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
              <span className="light-calibration-part-label">Included frames graded Suspect</span>
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
          {kind.alike && (
            <p className="light-calibration-part light-calibration-alike" data-testid="calibration-flat-alike">
              Measured alike: dust within {fractionPercent(kind.alike.dust)}, pattern within{' '}
              {fractionPercent(kind.alike.broad)} across {plural(kind.alike.nights, 'night')}
            </p>
          )}
          {kind.matching_unused.length > 0 && (
            <div className="light-calibration-part">
              <span className="light-calibration-part-label">Also match, not used</span>
              <ul>{kind.matching_unused.map((night) => nightRow(night, 'unused'))}</ul>
            </div>
          )}
          {kind.family && (
            <p className="light-calibration-part" data-testid="calibration-flat-family">
              <span className="light-calibration-part-label">Flat family</span>
              <span>
                {kind.family.label}
                {' · '}
                <span className="light-calibration-muted">{familyBasisLabel(kind.family)}</span>
                {kind.family.alike_range && (
                  <>
                    {' · '}
                    <span className="light-calibration-muted" data-testid="calibration-flat-alike-range">
                      flats at {kind.family.alike_range.flats_angle.toFixed(1)}°, measured alike at{' '}
                      {kind.family.alike_range.label} ({plural(kind.family.alike_range.pairs, 'pair')})
                    </span>
                  </>
                )}
              </span>
            </p>
          )}
          {kind.chosen_by && (
            <p
              className={`light-calibration-part light-calibration-chosen-by${
                (kind.basis && UNSETTLED_BASES.has(kind.basis.basis)) || kind.family?.by === 'disputed'
                  ? ' is-check'
                  : ''
              }`}
              data-testid="calibration-flat-chosen-by"
            >
              <span className="light-calibration-part-label">Chosen by</span>
              <span>{kind.chosen_by}</span>
            </p>
          )}
          {extra}
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
