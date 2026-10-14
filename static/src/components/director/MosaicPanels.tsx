import { Lock, LockOpen, Redo2, Trash2, Undo2 } from 'lucide-react';
import type { DirectorLayout, DirectorLayoutIssue } from '../../api/directorTypes';
import NumberInput from '../NumberInput';
import { DIRECTIONS, DIRECTION_NAMES, asGrid, describeIssue, findPanel, gridLayout, layoutCells, type Direction } from './mosaicLayout';

/** Where an extension starts: the chosen panel, or every panel on the
 *  mosaic's edge that way. */
export type ExtendScope = 'panel' | 'mosaic';

export interface MosaicPanelsProps {
  layout: DirectorLayout;
  issues: DirectorLayoutIssue[];
  selected: string | null;
  onSelect: (id: string | null) => void;
  /** Frames captured per panel, from the rigs that shoot this framing. */
  frames: (id: string) => number;
  /** Locked by hand or by its frames. */
  isLocked: (id: string) => boolean;
  canWrite: boolean;
  /** Replace the layout as one undoable step; `what` is said in the notice. */
  onEdit: (layout: DirectorLayout, what: string) => void;
  onExtend: (direction: Direction) => void;
  onRing: () => void;
  onFill: () => void;
  scope: ExtendScope;
  onScope: (scope: ExtendScope) => void;
  overlap: number;
  onOverlap: (percent: number) => void;
  undo: { can: boolean; run: () => void };
  redo: { can: boolean; run: () => void };
  /** The server would refuse the panels as they are. */
  invalid: boolean;
  /** What the last panel edit did, or why it did not. */
  notice: string;
  /** Why the grid cannot be laid out again, such as a locked panel. */
  gridBlocked: string | null;
}

const clamp = (value: number, low: number, high: number) => Math.max(low, Math.min(high, value));

/** The panels of the shared framing: a grid laid out by rows and columns,
 *  or panels placed one by one around those that are locked; the chosen
 *  panel and how to add more beside it or around the whole mosaic. */
export default function MosaicPanels(props: MosaicPanelsProps) {
  const { layout, issues, selected, onSelect, frames, isLocked, canWrite, onEdit, scope, overlap } = props;
  const grid = asGrid(layout);
  const chosen = selected ? findPanel(layout, selected) : undefined;
  const locked = layout.panels.filter(panel => isLocked(panel.id));
  const cells = layoutCells(layout);
  const ordered = layout.panels.map((panel, index) => ({ panel, cell: cells[index] })).sort((a, b) => a.cell[0] - b.cell[0] || a.cell[1] - b.cell[1]);
  const flagged = new Map<string, string[]>();
  for (const issue of issues) for (const id of issue.kind === 'disconnected' ? [] : issue.panels) flagged.set(id, [...(flagged.get(id) ?? []), describeIssue(issue)]);
  const fromChosen = scope === 'panel' && chosen;
  const where = (direction: Direction) => fromChosen ? `${DIRECTION_NAMES[direction]} of ${chosen.id}` : `on the mosaic's ${DIRECTION_NAMES[direction]} side`;
  const setGrid = (rows: number, columns: number, overlapPercent: number) => onEdit(gridLayout({ rows: clamp(Math.round(rows), 1, 16), columns: clamp(Math.round(columns), 1, 16), overlap_percent: clamp(Math.round(overlapPercent), 0, 90) }), 'Laid the panels out as a grid.');
  const number = (value: string, fallback: number) => { const parsed = Number(value); return Number.isFinite(parsed) ? parsed : fallback; };
  const rowsNow = Math.max(...cells.map(cell => cell[0]), 1);
  const columnsNow = Math.max(...cells.map(cell => cell[1]), 1);
  return <div className="framing-mosaic" data-testid="framing-mosaic">
    {grid && !props.gridBlocked
      ? <div className="framing-grid">
          <label>Rows<NumberInput aria-label="Mosaic rows" aria-invalid={props.invalid || undefined} min={1} max={16} step={1} value={grid.rows} disabled={!canWrite} onChange={event => setGrid(number(event.target.value, grid.rows), grid.columns, grid.overlap_percent)} /></label>
          <label>Columns<NumberInput aria-label="Mosaic columns" aria-invalid={props.invalid || undefined} min={1} max={16} step={1} value={grid.columns} disabled={!canWrite} onChange={event => setGrid(grid.rows, number(event.target.value, grid.columns), grid.overlap_percent)} /></label>
          <label>Overlap<span className="framing-input"><NumberInput aria-label="Panel overlap percent" aria-invalid={props.invalid || undefined} min={0} max={90} step={1} value={grid.overlap_percent} disabled={!canWrite} onChange={event => setGrid(grid.rows, grid.columns, number(event.target.value, grid.overlap_percent))} /><small>%</small></span></label>
        </div>
      : <div className="framing-grid">
          <label>Minimum overlap<span className="framing-input"><NumberInput aria-label="Minimum panel overlap percent" min={0} max={90} step={1} value={layout.min_overlap_percent} disabled={!canWrite}
            onChange={event => onEdit({ ...layout, min_overlap_percent: clamp(Math.round(number(event.target.value, layout.min_overlap_percent)), 0, 90) }, 'Changed the minimum overlap.')} /><small>%</small></span></label>
        </div>}
    {!grid && canWrite && <details className="framing-regrid">
      <summary>Lay out as a grid instead</summary>
      {props.gridBlocked
        ? <p className="director-muted">{props.gridBlocked}</p>
        : <p className="director-muted">Replaces every panel with a grid of {rowsNow} × {columnsNow} at {layout.min_overlap_percent}% overlap. <button type="button" className="link-button" onClick={() => setGrid(rowsNow, columnsNow, layout.min_overlap_percent)}>Replace with grid</button></p>}
    </details>}
    {grid && props.gridBlocked && <p className="director-muted framing-mosaic-note">{props.gridBlocked}</p>}

    <div className="framing-extend" role="group" aria-label="Add panels">
      <div className="framing-extend-head">
        <span className="framing-catalogs-title">Add panels</span>
        <div className="framing-scope" role="group" aria-label="Add beside">
          <button type="button" aria-pressed={scope === 'panel'} disabled={!chosen} title={chosen ? `Beside ${chosen.id}` : 'Choose a panel on the sky or in the list'} onClick={() => props.onScope('panel')}>{chosen ? `Beside ${chosen.id}` : 'Beside a panel'}</button>
          <button type="button" aria-pressed={scope === 'mosaic' || !chosen} onClick={() => props.onScope('mosaic')}>Whole mosaic</button>
        </div>
      </div>
      <div className="framing-extend-body">
        <div className="framing-pad" role="group" aria-label="Direction">
          {DIRECTIONS.slice(0, 4).map(direction => <button key={direction} type="button" disabled={!canWrite} aria-label={`Add ${fromChosen ? 'a panel' : direction.length === 2 ? 'panels' : direction === 'N' || direction === 'S' ? 'a row' : 'a column'} ${where(direction)}`} onClick={() => props.onExtend(direction)}>{direction}</button>)}
          <span className="framing-pad-center" aria-hidden="true">{fromChosen ? chosen.id : 'all'}</span>
          {DIRECTIONS.slice(4).map(direction => <button key={direction} type="button" disabled={!canWrite} aria-label={`Add ${fromChosen ? 'a panel' : direction.length === 2 ? 'panels' : direction === 'N' || direction === 'S' ? 'a row' : 'a column'} ${where(direction)}`} onClick={() => props.onExtend(direction)}>{direction}</button>)}
        </div>
        <div className="framing-extend-side">
          <label>Overlap<span className="framing-input"><NumberInput aria-label="Overlap of added panels percent" min={layout.min_overlap_percent} max={90} step={1} value={overlap} disabled={!canWrite} onChange={event => props.onOverlap(clamp(Math.round(number(event.target.value, overlap)), layout.min_overlap_percent, 90))} /><small>%</small></span></label>
          <button type="button" disabled={!canWrite} title="A panel on every side of the whole mosaic, corners too" onClick={props.onRing}>Ring around</button>
          <button type="button" disabled={!canWrite || layout.panels.length < 3} title="Fill the holes in the mosaic's rectangle" onClick={props.onFill}>Fill holes</button>
        </div>
      </div>
      <p className="director-muted framing-mosaic-note">N, E, S and W are on the sky: each adds on the panel side facing nearest that way at this camera angle. Panels go in at {overlap}% overlap.</p>
    </div>

    {props.notice && <p className="framing-mosaic-notice" role="status" data-testid="framing-mosaic-notice">{props.notice}</p>}
    <div className="framing-panel-head">
      <span className="framing-catalogs-title">{layout.panels.length} panel{layout.panels.length === 1 ? '' : 's'}{locked.length > 0 ? ` · ${locked.length} locked` : ''}</span>
      <span className="framing-history">
        <button type="button" aria-label="Undo panel change" title="Undo (Ctrl+Z)" disabled={!props.undo.can} onClick={props.undo.run}><Undo2 size={15} /></button>
        <button type="button" aria-label="Redo panel change" title="Redo (Ctrl+Shift+Z)" disabled={!props.redo.can} onClick={props.redo.run}><Redo2 size={15} /></button>
      </span>
    </div>
    <ul className="framing-panel-list" aria-label="Panels">
      {ordered.map(({ panel }) => {
        const held = isLocked(panel.id);
        const shot = frames(panel.id);
        const problems = flagged.get(panel.id) ?? [];
        return <li key={panel.id}>
          <button type="button" className="framing-panel-pick" aria-pressed={panel.id === selected} aria-invalid={problems.length > 0 || undefined}
            data-panel-state={held ? 'locked' : problems.length > 0 ? 'invalid' : undefined} title={problems.join('; ') || undefined}
            onClick={() => onSelect(panel.id === selected ? null : panel.id)}>
            <strong>{panel.id}</strong>
            {held && <Lock size={13} aria-label="locked" />}
            <small>{shot > 0 ? `${shot} frame${shot === 1 ? '' : 's'}` : held ? 'locked' : problems.length > 0 ? 'check overlap' : ''}</small>
          </button>
        </li>;
      })}
    </ul>
    {chosen && <div className="framing-chosen" data-testid="framing-chosen">
      <p>
        <strong>{chosen.id}</strong>{' '}
        {frames(chosen.id) > 0
          ? <span className="director-muted">Locked: its {frames(chosen.id)} frames were shot here.</span>
          : chosen.locked ? <span className="director-muted">Locked by hand.</span> : <span className="director-muted">Drag it on the sky with Panel, or nudge it with the arrow keys.</span>}
      </p>
      {canWrite && <div className="director-actions">
        {frames(chosen.id) === 0 && <button type="button" aria-pressed={chosen.locked} onClick={() => onEdit({ ...layout, panels: layout.panels.map(panel => panel.id === chosen.id ? { ...panel, locked: !panel.locked } : panel) }, chosen.locked ? `Unlocked ${chosen.id}.` : `Locked ${chosen.id}.`)}>
          {chosen.locked ? <><LockOpen size={15} />Unlock</> : <><Lock size={15} />Lock</>}</button>}
        <button type="button" disabled={isLocked(chosen.id) || layout.panels.length < 2} title={isLocked(chosen.id) ? 'Unlock it first' : undefined}
          onClick={() => { onEdit({ ...layout, panels: layout.panels.filter(panel => panel.id !== chosen.id) }, `Removed ${chosen.id}.`); onSelect(null); }}><Trash2 size={15} />Remove</button>
      </div>}
    </div>}
    {issues.length > 0 && <ul className="framing-issues" data-testid="framing-issues" aria-label="Panel problems">
      {issues.map(issue => <li key={`${issue.kind}:${issue.panels.join(',')}`}>{describeIssue(issue)}{issue.kind === 'thin_overlap' ? `, under the ${layout.min_overlap_percent}% minimum` : ''}.</li>)}
    </ul>}
  </div>;
}
