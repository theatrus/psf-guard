import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent, type ReactNode } from 'react';
import { ArrowDown, ArrowUp, X } from 'lucide-react';
import type { DirectorScheduleGoal, NightInterval, NightOverride, ScheduleBlock } from '../../api/nightPlanTypes';
import { formatClock } from './nightsModel';
import { describeOverride, leaveOut, pinBlock, subjectName, subjectsOf, withHold, withWindow } from './overridesModel';

/** How far one arrow key press moves a band, an edge or the hold. */
export const STEP_MS = 15 * 60_000;
/** Drags round to this, so what is dragged lands on a clock time. */
export const SNAP_MS = 5 * 60_000;
/** The shortest pin or exclusion a drag leaves. */
const SHORTEST_MS = 10 * 60_000;

/** Where an instant sits on the drawn span, and the time a drag covers. */
export interface TimelineScale {
  start: number;
  end: number;
  /** `left` and `width` styles for a stretch, `null` when it is off the span. */
  place: (from: number, to: number) => { left: string; width: string } | null;
  /** The milliseconds a drag of `dx` pixels covers, rounded to the snap. */
  toMs: (dx: number) => number;
}

/** What the timeline edits: a Director night's overrides as edited, and
 *  what each change does to them. */
export interface TimelineEdit {
  goals: DirectorScheduleGoal[];
  entries: NightOverride[];
  /** The night, noon to noon. */
  night: NightInterval;
  /** When the dark ends: a hold at or after it holds nothing. */
  dawn: number;
  zone?: string;
  onChange: (entries: NightOverride[]) => void;
  onMoveBlock: (block: ScheduleBlock, deltaMs: number) => void;
}

const snap = (ms: number) => Math.round(ms / SNAP_MS) * SNAP_MS;

/** A small menu under what opened it. Escape or a click elsewhere closes
 *  it; the arrow keys move between its items. */
export function TimelineMenu({ label, left, onClose, children }: { label: string; left: string; onClose: () => void; children: ReactNode }) {
  const root = useRef<HTMLDivElement>(null);
  useEffect(() => {
    root.current?.querySelector<HTMLElement>('[role="menuitem"]:not([disabled])')?.focus();
    const onPointer = (event: globalThis.PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) onClose();
    };
    document.addEventListener('pointerdown', onPointer);
    return () => document.removeEventListener('pointerdown', onPointer);
  }, [onClose]);
  const keys = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === 'Escape') {
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key !== 'ArrowDown' && event.key !== 'ArrowUp') return;
    const items = Array.from(root.current?.querySelectorAll<HTMLElement>('[role="menuitem"]:not([disabled])') ?? []);
    if (items.length === 0) return;
    event.preventDefault();
    const at = items.indexOf(document.activeElement as HTMLElement);
    items[(at + (event.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length].focus();
  };
  return <div ref={root} className="nights-menu" role="menu" aria-label={label} style={{ left }} onKeyDown={keys}>
    <p className="nights-menu-title">{label}</p>
    {children}
  </div>;
}

/** The menu a block opens while the night is edited. */
export function BlockMenu({ block, edit, left, onReasons, onClose }: { block: ScheduleBlock; edit: TimelineEdit; left: string; onReasons: () => void; onClose: () => void }) {
  const clock = (ms: number) => formatClock(ms, edit.zone);
  const frames = block.runs.reduce((sum, run) => sum + run.frames, 0);
  const pin = pinBlock(block, edit.goals);
  const from = leaveOut(block, edit.goals, edit.night, 'block');
  const night = leaveOut(block, edit.goals, edit.night, 'night');
  const act = (change: () => void) => () => { onClose(); change(); };
  const add = (entry: NightOverride | null) => act(() => { if (entry) edit.onChange([...edit.entries, entry]); });
  return <TimelineMenu label={`${block.target.target_name}, ${clock(block.start_ms)} – ${clock(block.end_ms)}, ${frames} frames`} left={left} onClose={onClose}>
    <button type="button" role="menuitem" disabled={!pin} onClick={add(pin)}>Pin it here</button>
    <button type="button" role="menuitem" disabled={!from} onClick={add(from)}>Leave it out from {clock(block.start_ms)}</button>
    <button type="button" role="menuitem" disabled={!night} onClick={add(night)}>Leave it out tonight</button>
    <button type="button" role="menuitem" onClick={act(() => edit.onMoveBlock(block, 30 * 60_000))}>Move it 30 minutes later</button>
    <button type="button" role="menuitem" onClick={act(() => edit.onMoveBlock(block, -30 * 60_000))}>Move it 30 minutes earlier</button>
    <button type="button" role="menuitem" onClick={act(onReasons)}>Why this target</button>
  </TimelineMenu>;
}

/** The night's pins and windowed exclusions as bands to drag, resize or
 *  remove, above the schedule. */
export function EditableBands({ edit, scale }: { edit: TimelineEdit; scale: TimelineScale }) {
  const [drag, setDrag] = useState<{ index: number; part: 'move' | 'start' | 'end'; x: number; dx: number } | null>(null);
  const [menu, setMenu] = useState<number | null>(null);
  const moved = useRef(false);
  const clock = (ms: number) => formatClock(ms, edit.zone);
  // A window after a drag or a key: inside the night, and long enough.
  const shifted = (window: NightInterval, part: 'move' | 'start' | 'end', delta: number): NightInterval => {
    const length = window.end_ms - window.start_ms;
    if (part === 'move') {
      const start = Math.min(Math.max(window.start_ms + delta, edit.night.start_ms), edit.night.end_ms - length);
      return { start_ms: start, end_ms: start + length };
    }
    if (part === 'start') return { start_ms: Math.min(Math.max(window.start_ms + delta, edit.night.start_ms), window.end_ms - SHORTEST_MS), end_ms: window.end_ms };
    return { start_ms: window.start_ms, end_ms: Math.max(Math.min(window.end_ms + delta, edit.night.end_ms), window.start_ms + SHORTEST_MS) };
  };
  const press = (index: number, part: 'move' | 'start' | 'end') => (event: PointerEvent<HTMLElement>) => {
    event.stopPropagation();
    event.currentTarget.setPointerCapture?.(event.pointerId);
    setDrag({ index, part, x: event.clientX, dx: 0 });
  };
  const slide = (event: PointerEvent<HTMLElement>) => { if (drag) setDrag({ ...drag, dx: event.clientX - drag.x }); };
  const release = () => {
    if (!drag) return;
    const entry = edit.entries[drag.index];
    const delta = scale.toMs(drag.dx);
    setDrag(null);
    moved.current = delta !== 0;
    if (delta !== 0 && (entry.kind === 'pin' || entry.kind === 'exclude') && entry.window) {
      edit.onChange(withWindow(edit.entries, drag.index, shifted(entry.window, drag.part, delta)));
    }
  };
  const nudge = (index: number, part: 'move' | 'start' | 'end') => (event: KeyboardEvent<HTMLElement>) => {
    const entry = edit.entries[index];
    if ((event.key !== 'ArrowRight' && event.key !== 'ArrowLeft') || (entry.kind !== 'pin' && entry.kind !== 'exclude') || !entry.window) return;
    event.preventDefault();
    edit.onChange(withWindow(edit.entries, index, shifted(entry.window, part, event.key === 'ArrowRight' ? STEP_MS : -STEP_MS)));
  };
  return <div className="nights-overrides is-editing" role="group" aria-label="Overrides">
    {edit.entries.map((entry, index) => {
      if ((entry.kind !== 'pin' && entry.kind !== 'exclude') || !entry.window) return null;
      const window = drag?.index === index ? shifted(entry.window, drag.part, scale.toMs(drag.dx)) : entry.window;
      const style = scale.place(window.start_ms, window.end_ms);
      if (!style) return null;
      const words = describeOverride({ ...entry, window }, edit.goals, edit.zone);
      const name = subjectName(entry.subject, edit.goals);
      return <div key={`${entry.kind}-${index}`} className="nights-override" data-kind={entry.kind} style={style}>
        <span className="nights-override-edge" role="slider" tabIndex={0} aria-label={`Start of: ${words}`}
          aria-valuemin={edit.night.start_ms} aria-valuemax={window.end_ms} aria-valuenow={window.start_ms} aria-valuetext={clock(window.start_ms)}
          onPointerDown={press(index, 'start')} onPointerMove={slide} onPointerUp={release} onPointerCancel={() => setDrag(null)} onKeyDown={nudge(index, 'start')} />
        <button type="button" className="nights-override-body" title={`${words}. Drag to move it, drag an edge to change its times, or choose it for more.`}
          aria-label={`${words}. Press the left or right arrow key to move it 15 minutes.`} aria-haspopup="menu" aria-expanded={menu === index}
          onPointerDown={press(index, 'move')} onPointerMove={slide} onPointerUp={release} onPointerCancel={() => setDrag(null)} onKeyDown={nudge(index, 'move')}
          onClick={() => { if (moved.current) moved.current = false; else setMenu(menu === index ? null : index); }}>
          {entry.kind === 'pin' ? `Only ${name}` : `Not ${name}`}
        </button>
        <span className="nights-override-edge" role="slider" tabIndex={0} aria-label={`End of: ${words}`}
          aria-valuemin={window.start_ms} aria-valuemax={edit.night.end_ms} aria-valuenow={window.end_ms} aria-valuetext={clock(window.end_ms)}
          onPointerDown={press(index, 'end')} onPointerMove={slide} onPointerUp={release} onPointerCancel={() => setDrag(null)} onKeyDown={nudge(index, 'end')} />
        {menu === index && <TimelineMenu label={words} left="0" onClose={() => setMenu(null)}>
          <button type="button" role="menuitem" onClick={() => { setMenu(null); edit.onChange(edit.entries.filter((_, at) => at !== index)); }}>
            Remove this {entry.kind === 'pin' ? 'pin' : 'exclusion'}
          </button>
        </TimelineMenu>}
      </div>;
    })}
  </div>;
}

/**
 * The hold line: from where it stands the rig takes no more work. At the
 * end of the dark it holds nothing. Drag it in from the right, or use the
 * arrow keys: End lifts the hold, Home holds the whole night.
 */
export function HoldHandle({ edit, scale, width }: { edit: TimelineEdit; scale: TimelineScale; width: () => number }) {
  const hold = edit.entries.find(entry => entry.kind === 'hold');
  const from = hold?.kind === 'hold' ? hold.from_ms ?? edit.night.start_ms : null;
  // Where the drag began, and where the line is now.
  const [drag, setDrag] = useState<{ x: number; from: number; at: number } | null>(null);
  const clock = (ms: number) => formatClock(ms, edit.zone);
  const shown = drag ? drag.at : from ?? edit.dawn;
  const clamp = (at: number) => Math.min(Math.max(at, scale.start), edit.dawn);
  // At the end of the dark, or past it, holds nothing.
  const set = (at: number | null) => edit.onChange(withHold(edit.entries, at === null || at >= edit.dawn ? null : at));
  const left = `${Math.min(1, Math.max(0, (shown - scale.start) / (scale.end - scale.start))) * 100}%`;
  return <span className={`nights-hold${from !== null || drag ? ' is-set' : ''}`} style={{ left }} role="slider" tabIndex={0}
    aria-label="Hold the rig from" aria-valuemin={scale.start} aria-valuemax={edit.dawn} aria-valuenow={shown}
    aria-valuetext={from === null && !drag ? 'No hold' : `Hold from ${clock(shown)}`}
    title={from === null ? 'Drag left to hold the rig from a time' : `Holds the rig from ${clock(from)}. Drag right to the end of the dark to lift it.`}
    onPointerDown={event => {
      event.currentTarget.setPointerCapture?.(event.pointerId);
      setDrag({ x: event.clientX, from: shown, at: shown });
    }}
    onPointerMove={event => {
      if (!drag || width() <= 0) return;
      setDrag({ ...drag, at: clamp(snap(drag.from + ((event.clientX - drag.x) / width()) * (scale.end - scale.start))) });
    }}
    onPointerUp={() => { if (drag) { set(drag.at); setDrag(null); } }}
    onPointerCancel={() => setDrag(null)}
    onKeyDown={event => {
      const keys: Record<string, number | null> = {
        ArrowLeft: clamp((from ?? edit.dawn) - STEP_MS),
        ArrowRight: from === null ? null : from + STEP_MS,
        Home: edit.night.start_ms,
        End: null,
      };
      if (!(event.key in keys)) return;
      event.preventDefault();
      set(keys[event.key]);
    }}>
    <span className="nights-hold-label" aria-hidden="true">{from === null && !drag ? 'Hold' : `Held from ${clock(shown)}`}</span>
  </span>;
}

/** What the timeline cannot draw: plans and targets left out all night, and
 *  the night's plan order. While the night is edited each can be removed,
 *  and the order set. */
export function OverrideChips({ entries, goals, zone, editing, onChange }: { entries: NightOverride[]; goals: DirectorScheduleGoal[]; zone?: string; editing: boolean; onChange: (entries: NightOverride[]) => void }) {
  const [ordering, setOrdering] = useState(false);
  const order = entries.find(entry => entry.kind === 'order');
  const chips = entries.map((entry, index) => ({ entry, index }))
    .filter(({ entry }) => (entry.kind === 'exclude' && !entry.window) || entry.kind === 'order');
  if (chips.length === 0 && !editing) return null;
  return <div className="nights-chips" role="group" aria-label="Overrides for the whole night">
    {chips.map(({ entry, index }) => <span key={`${entry.kind}-${index}`} className="nights-chip-override">
      {describeOverride(entry, goals, zone)}
      {editing && <button type="button" aria-label={`Remove: ${describeOverride(entry, goals, zone)}`} title="Remove" onClick={() => onChange(entries.filter((_, at) => at !== index))}><X size={12} /></button>}
    </span>)}
    {editing && <span className="nights-chip-order">
      <button type="button" aria-haspopup="menu" aria-expanded={ordering} onClick={() => setOrdering(!ordering)}>{order ? 'Change the plan order' : 'Set a plan order'}</button>
      {ordering && <OrderMenu goals={goals} current={order?.kind === 'order' ? order.plans : null}
        onClose={() => setOrdering(false)}
        onApply={plans => { setOrdering(false); onChange([...entries.filter(entry => entry.kind !== 'order'), { kind: 'order', plans }]); }} />}
    </span>}
  </div>;
}

function OrderMenu({ goals, current, onApply, onClose }: { goals: DirectorScheduleGoal[]; current: string[] | null; onApply: (plans: string[]) => void; onClose: () => void }) {
  const plans = subjectsOf(goals).plans.map(plan => plan.subject.kind === 'plan' ? plan.subject.plan_id : '');
  const names = new Map(subjectsOf(goals).plans.map(plan => [plan.subject.kind === 'plan' ? plan.subject.plan_id : '', plan.name]));
  const [order, setOrder] = useState(() => current ? [...current.filter(id => plans.includes(id)), ...plans.filter(id => !current.includes(id))] : plans);
  const move = (index: number, by: number) => {
    const next = [...order];
    const [item] = next.splice(index, 1);
    next.splice(index + by, 0, item);
    setOrder(next);
  };
  return <TimelineMenu label="Tonight's plan order, first first" left="0" onClose={onClose}>
    <ol className="nights-order">
      {order.map((plan, index) => <li key={plan}>
        <span>{names.get(plan) ?? plan}</span>
        <button type="button" aria-label={`Move ${names.get(plan) ?? plan} up`} title="Up" disabled={index === 0} onClick={() => move(index, -1)}><ArrowUp size={14} /></button>
        <button type="button" aria-label={`Move ${names.get(plan) ?? plan} down`} title="Down" disabled={index === order.length - 1} onClick={() => move(index, 1)}><ArrowDown size={14} /></button>
      </li>)}
    </ol>
    <button type="button" role="menuitem" disabled={order.length < 2} onClick={() => onApply(order)}>Use this order tonight</button>
  </TimelineMenu>;
}
