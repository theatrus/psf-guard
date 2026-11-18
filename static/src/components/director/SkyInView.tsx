import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { openSettings } from '../../utils/settingsIntent';
import { SkyChoice } from '../sky/SkyChoices';
import { coverImage } from '../sky/skyShade';
import { formatMinutes } from '../sky/visibleSky';
import type { Stage, StageView } from './framingModel';
import { COVER_REST_MS, coverSize, stageLines, stagePlace } from './skyInViewStage';
import { COVER_ACROSS, COVER_ACROSS_MOVING, MAX_MINUTES, SEASON_PAGE, SPANS, type SkyInViewModel } from './useSkyInView';
import './SkyInView.css';

/**
 * The overlay on Framing's stage: the cover and the two lines, in the
 * stage's own projection. The cover is a small picture of the stage whose
 * pixels are white; how solid each is carries how much of the sky under
 * it is covered, and the stylesheet gives it its colour. It is made on an
 * animation frame, so however often the view moves in a frame it is made
 * once, and handed straight to its element.
 *
 * Whatever changes what the cover shows, the view's middle, its zoom, the
 * turn of the sky, the stage's size or the counts under a slider, it is
 * first made small, and at full size once nothing has changed for a
 * moment: a full picture at every frame of a drag would hold the drag up.
 */
export function SkyInViewLayer({ layer, view, viewFov, stage }: { layer: NonNullable<SkyInViewModel['layer']>; view: StageView; viewFov: number; stage: Stage }) {
  const maskId = useId();
  const image = useRef<SVGImageElement>(null);
  const { counts, lines } = layer;
  // What the full-size cover on screen was made from: the counts, and
  // the view by its numbers. Anything else is still moving.
  const viewKey = [view.anchor.ra_degrees, view.anchor.dec_degrees, view.offset[0], view.offset[1], view.rotation, viewFov, stage.width, stage.height].join(' ');
  const [rested, setRested] = useState<{ counts: typeof counts; viewKey: string } | null>(null);
  const atRest = rested !== null && rested.counts === counts && rested.viewKey === viewKey;
  useEffect(() => {
    if (atRest) return;
    const timer = window.setTimeout(() => setRested({ counts, viewKey }), COVER_REST_MS);
    return () => window.clearTimeout(timer);
  }, [counts, viewKey, atRest]);
  useEffect(() => {
    const [width, height] = coverSize(stage, atRest ? COVER_ACROSS : COVER_ACROSS_MOVING);
    const paint = () => {
      const url = coverImage(counts, stage.width, stage.height, width, height, stagePlace(view, viewFov, stage));
      if (url) image.current?.setAttribute('href', url);
      else image.current?.removeAttribute('href');
    };
    if (typeof requestAnimationFrame !== 'function') {
      paint();
      return;
    }
    const frame = requestAnimationFrame(paint);
    return () => cancelAnimationFrame(frame);
  }, [counts, view, viewFov, stage, atRest]);
  const every = useMemo(() => stageLines(lines.every, view, viewFov, stage), [lines.every, view, viewFov, stage]);
  const any = useMemo(() => (lines.any ? stageLines(lines.any, view, viewFov, stage) : null), [lines.any, view, viewFov, stage]);
  return (
    <g className="sky-in-view-layer" data-testid="sky-in-view-layer" data-nights={counts.nights}>
      <mask id={maskId} maskUnits="userSpaceOnUse" x={0} y={0} width={stage.width} height={stage.height}>
        <image ref={image} x={0} y={0} width={stage.width} height={stage.height} preserveAspectRatio="none" />
      </mask>
      <rect className="sky-in-view-cover" x={0} y={0} width={stage.width} height={stage.height} mask={`url(#${maskId})`} />
      {any && (
        <>
          <path className="sky-in-view-line sky-in-view-any" d={any.line} />
          <path className="sky-in-view-marks sky-in-view-any" d={any.marks} />
        </>
      )}
      <path className="sky-in-view-line sky-in-view-every" d={every.line} />
      <path className="sky-in-view-marks sky-in-view-every" d={every.marks} />
    </g>
  );
}

/**
 * The overlay's words and controls, a group among Framing's own. The
 * target's nights in view are always in sight, on lines kept for them so
 * nothing under them moves as they change. The time frame, the minimum
 * time, Season's slider and the key are folded behind one line. Nothing
 * here is read out as it changes: the slider speaks its own value.
 */
export function SkyInViewPanel({ model, onShow }: { model: SkyInViewModel; onShow: (on: boolean) => void }) {
  const { panel, rig } = model;
  const sliderId = useId();
  const season = panel.season;
  return (
    <fieldset className="sky-in-view" data-testid="sky-in-view" aria-busy={panel.busy}>
      <legend>Sky in view{rig ? ` · ${rig.name}` : ''}</legend>
      <p className="sky-in-view-status" data-testid="sky-in-view-status" title={panel.status} data-stale={panel.stale ? 'true' : undefined}>{panel.status}</p>
      {panel.failure?.needsSite && rig && (
        <button type="button" className="link-button" onClick={() => openSettings('rigs', { databaseId: rig.slug ?? undefined })}>
          Open Rig setup
        </button>
      )}
      <label className="sky-in-view-show">
        <input type="checkbox" checked={model.on} onChange={(event) => onShow(event.target.checked)} />
        Show on the sky
      </label>
      {model.on && rig && (
        <details className="sky-in-view-more">
          <summary>Time range and overlay key</summary>
          <div className="sky-in-view-body">
            <div className="sky-in-view-controls">
              <SkyChoice label="Time frame" options={SPANS} value={panel.span} onChange={panel.setSpan} />
              <span className="sky-in-view-minimum">
                In view for
                <input
                  type="number"
                  aria-label="Minimum minutes"
                  min={1}
                  max={MAX_MINUTES}
                  step={5}
                  value={panel.typed}
                  onChange={(event) => panel.typeMinutes(event.target.value)}
                  onBlur={(event) => panel.sendMinutes(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key !== 'Enter') return;
                    // Not the form's Save.
                    event.preventDefault();
                    panel.sendMinutes(event.currentTarget.value);
                  }}
                />
                min or more
              </span>
              {panel.rigMinimum !== null && (
                <button type="button" className="link-button" onClick={panel.useRigMinimum}>
                  Use rig's {formatMinutes(panel.rigMinimum)}
                </button>
              )}
            </div>
            {season && (
              <div className="sky-in-view-season">
                <label htmlFor={sliderId}>Time of year</label>
                <span className="sky-in-view-season-nights">{season.from ?? 'Working out the year…'}</span>
                <input
                  id={sliderId}
                  type="range"
                  min={0}
                  max={season.last}
                  step={1}
                  value={season.position}
                  disabled={season.from === null}
                  aria-valuetext={season.from ?? undefined}
                  onChange={(event) => season.slideTo(Number(event.target.value))}
                  onKeyDown={(event) => {
                    // Arrow keys step a week, and Home and End go to the
                    // ends, as a slider does; a page is about a month.
                    if (event.key !== 'PageUp' && event.key !== 'PageDown') return;
                    event.preventDefault();
                    season.slideTo(season.position + (event.key === 'PageUp' ? SEASON_PAGE : -SEASON_PAGE));
                  }}
                />
                <div className="sky-in-view-months" aria-hidden="true">
                  <div>
                    {season.months.map((mark) => (
                      <span key={mark.label + mark.share} style={{ left: `${mark.share * 100}%` }}>
                        {mark.label}
                      </span>
                    ))}
                  </div>
                </div>
              </div>
            )}
            {panel.legend && <p className="sky-in-view-text" data-testid="sky-in-view-legend">{panel.legend}</p>}
            {panel.shares && <p className="sky-in-view-text" data-testid="sky-in-view-shares">{panel.shares}</p>}
            {panel.viewWords && <p className="sky-in-view-text" data-testid="sky-in-view-view">{panel.viewWords}</p>}
            <p className="sky-in-view-key">
              <span className="sky-in-view-scale-end">Never</span>
              <span className="sky-in-view-scale" aria-hidden="true" />
              <span className="sky-in-view-scale-end">{panel.clearNights !== null ? `${panel.clearNights} nights or more` : 'Every night'}</span>
              <span><span className="sky-in-view-key-solid" aria-hidden="true" />{panel.clearNights !== null ? `In view on at least ${panel.clearNights} nights` : 'In view every night'}</span>
              <span><span className="sky-in-view-key-dashed" aria-hidden="true" />In view on at least one night</span>
            </p>
            <p className="sky-in-view-text">
              {panel.clearNights !== null ? `Shaded sky is in view on fewer than ${panel.clearNights} nights.` : 'Shaded sky is out of view on some nights.'} Hatched sky is never in view. Line markers point toward visible sky.
              {panel.span === 'season' && ' Season shows 31 nights at a time. Its boundaries follow a 2° grid and are less precise than Now and Year.'}
              {panel.year && ' Year estimates visibility for every night from weekly samples.'}
              {panel.year && panel.clearNights !== null && ` Unshaded sky is in view on at least ${panel.clearNights} nights; they need not be consecutive. The solid boundary is within about 1° of a boundary calculated for every night.`}
            </p>
          </div>
        </details>
      )}
    </fieldset>
  );
}
