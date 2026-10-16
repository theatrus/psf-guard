import type {
  CalibrationFlatFeature,
  CalibrationVignetting,
  DustShape,
  FlatSpot,
  FlatVignettingChange,
  VignettingEdge,
} from '../../api/types';

/** How a mark is drawn: by shape over one flat, by what happened between
 *  two sessions, or as a vignetting shadow. */
export type DustTone =
  | 'spot'
  | 'disk'
  | 'donut'
  | 'transient'
  | 'new'
  | 'gone'
  | 'moved'
  | 'tolerance';

/** One dust feature to draw, in sensor pixels. */
export interface DustMark {
  key: string;
  shape: DustShape;
  x: number;
  y: number;
  radius: number;
  /** A donut's hole, as a fraction of `radius`. */
  hole?: number | null;
  /** Left, top, right, bottom: what a dip or a patch covers. */
  bounds: [number, number, number, number];
  tone: DustTone;
  /** A few words drawn beside a large mark. */
  label: string;
  /** Fewer, where the picture is drawn small. */
  short: string;
  /** The numbers, for a hover or a tap. */
  detail: string;
  /** Session dust this flat does not show: drawn dimmed. */
  unseen?: boolean;
}

/** A vignetting shadow to draw. */
export interface VignettingMark {
  key: string;
  edge: VignettingEdge;
  x: number;
  y: number;
  area: number;
  label: string;
  short: string;
  detail: string;
}

export const SHAPE_WORDS: Record<DustShape, string> = {
  compact: 'spot',
  irregular: 'irregular spot',
  disk: 'disk',
  donut: 'donut',
  patch: 'patch of disks',
};

const EDGE_WORDS: Record<VignettingEdge, string> = {
  top: 'top edge',
  bottom: 'bottom edge',
  left: 'left edge',
  right: 'right edge',
  top_left: 'top-left corner',
  top_right: 'top-right corner',
  bottom_left: 'bottom-left corner',
  bottom_right: 'bottom-right corner',
};

/** An out-of-focus shadow of dust up the train: a disk, a donut, or a patch
 *  of them that would not separate. */
export function isLarge(shape: DustShape | undefined): boolean {
  return shape === 'disk' || shape === 'donut' || shape === 'patch';
}

/** One shadow with a round rim: drawn as a circle at its radius. */
export function isRound(shape: DustShape | undefined): boolean {
  return shape === 'disk' || shape === 'donut';
}

/** Where dust that far in front of the sensor most likely sits. */
export function whereDust(distanceMm: number): string {
  if (distanceMm < 3) return 'sensor glass';
  if (distanceMm < 8) return 'camera window';
  if (distanceMm < 30) return 'filter/window';
  return 'further up the train';
}

const pct = (fraction: number) => `${(fraction * 100).toFixed(2)}%`;

/** `disk 0.33% · ~11 mm, filter/window` */
export function featureLabel(feature: CalibrationFlatFeature): string {
  const shape = SHAPE_WORDS[feature.shape ?? 'compact'];
  const where = feature.distance_mm !== undefined && feature.distance_mm !== null
    ? ` · ~${Math.round(feature.distance_mm)} mm, ${whereDust(feature.distance_mm)}`
    : '';
  return `${shape} ${pct(feature.depth)}${where}`;
}

/** Every number a feature carries, for a hover or a tap. */
export function featureDetail(feature: CalibrationFlatFeature, transient = false): string {
  const parts = [featureLabel(feature)];
  const across = isLarge(feature.shape) && feature.radius
    ? `${Math.round(2 * feature.radius)} px across`
    : `${feature.pixels.toLocaleString()} px`;
  parts.push(across);
  if (feature.shape === 'donut' && feature.hole) parts.push(`hole ${Math.round(feature.hole * 100)}%`);
  if (feature.seen_in) parts.push(`in ${feature.seen_in.frames} of ${feature.seen_in.of} flats`);
  parts.push(`at (${Math.round(feature.x)}, ${Math.round(feature.y)})`);
  if (transient) parts.push('this flat only: not dust');
  return parts.join(' · ');
}

/** A flat's own feature as a mark. */
export function featureMark(
  feature: CalibrationFlatFeature,
  key: string,
  options: { transient?: boolean; unseen?: boolean } = {},
): DustMark {
  const shape = feature.shape ?? 'compact';
  const tone: DustTone = options.transient
    ? 'transient'
    : shape === 'disk' || shape === 'patch' ? 'disk' : shape === 'donut' ? 'donut' : 'spot';
  const detail = featureDetail(feature, options.transient)
    + (options.unseen ? ' · not seen in this flat' : '');
  return {
    key,
    shape,
    x: feature.x,
    y: feature.y,
    radius: feature.radius ?? Math.sqrt(feature.pixels / Math.PI),
    hole: feature.hole,
    bounds: [feature.left, feature.top, feature.right, feature.bottom],
    tone,
    label: options.transient ? `${SHAPE_WORDS[shape]} · this flat only` : featureLabel(feature),
    short: pct(feature.depth),
    detail,
    unseen: options.unseen,
  };
}

const SPOT_KIND_WORDS: Record<FlatSpot['kind'], string> = {
  new: 'new',
  gone: 'gone',
  moved: 'moved',
};

/** A change of dust between two sessions as a mark. */
export function spotMark(spot: FlatSpot, key: string): DustMark {
  const shape = spot.shape ?? 'compact';
  const tone: DustTone = spot.within_tolerance ? 'tolerance' : spot.kind;
  const words = spot.within_tolerance
    ? `${SHAPE_WORDS[shape]} moved ${spot.offset_px ? `${Math.round(spot.offset_px)} px` : ''}, within focus travel`
    : `${SPOT_KIND_WORDS[spot.kind]} ${SHAPE_WORDS[shape]}`;
  const label = `${words.replace(/\s+/g, ' ').trim()} ${spot.peak_pct.toFixed(2)}%`;
  const bounds: [number, number, number, number] = spot.bounds
    ?? [spot.x - spot.radius, spot.y - spot.radius, spot.x + spot.radius, spot.y + spot.radius];
  return {
    key,
    shape,
    x: spot.x,
    y: spot.y,
    radius: spot.radius,
    hole: spot.hole,
    bounds,
    tone,
    label,
    short: `${spot.peak_pct.toFixed(2)}%`,
    detail: `${label} · ${spot.darker ? 'darker' : 'brighter'} · ${Math.round(2 * spot.radius)} px · at (${Math.round(spot.x)}, ${Math.round(spot.y)})`,
  };
}

/** What happened to a spot between two sessions, in a word or three:
 *  `New`, `Gone disk`, `Moved donut within focus travel`. */
export function spotWords(spot: FlatSpot): string {
  const kind = spot.within_tolerance ? 'Moved' : { new: 'New', gone: 'Gone', moved: 'Moved' }[spot.kind];
  const shape = isLarge(spot.shape) ? ` ${SHAPE_WORDS[spot.shape!]}` : '';
  return `${kind}${shape}${spot.within_tolerance ? ' within focus travel' : ''}`;
}

/** A flat's own vignetting as a mark. */
export function vignettingMark(v: CalibrationVignetting, key: string): VignettingMark {
  const label = `vignetting ${pct(v.depth)} · ${EDGE_WORDS[v.edge]}`;
  return { key, edge: v.edge, x: v.x, y: v.y, area: v.area, label, short: `vignetting ${pct(v.depth)}`, detail: `${label}: not dust` };
}

/** A change of vignetting between two sessions as a mark. */
export function vignettingChangeMark(v: FlatVignettingChange, key: string): VignettingMark {
  const label = `vignetting ${v.darker ? 'deeper' : 'lighter'} ${v.depth_pct.toFixed(2)}% · ${EDGE_WORDS[v.edge]}`;
  return {
    key, edge: v.edge, x: v.x, y: v.y, area: v.area, label,
    short: `vignetting ${v.depth_pct.toFixed(2)}%`,
    detail: `${label}: the flats differ, not dust`,
  };
}

/** `15 spots · 3 large disks`, from the counts a frame or session reads. */
export function spotsSummary(spots: number | null | undefined, large: number | null | undefined, largeShapes?: DustShape[]): string {
  if (spots === null || spots === undefined || !Number.isFinite(spots)) return '—';
  const count = (n: number) => Number.isInteger(n) ? n.toLocaleString() : n.toFixed(1);
  const spotText = `${count(spots)} ${spots === 1 ? 'spot' : 'spots'}`;
  if (!large) return spotText;
  const donuts = largeShapes && largeShapes.length > 0 && largeShapes.every((shape) => shape === 'donut');
  const noun = donuts ? (large === 1 ? 'donut' : 'donuts') : large === 1 ? 'large disk' : 'large disks';
  return `${spotText} · ${count(large)} ${noun}`;
}
