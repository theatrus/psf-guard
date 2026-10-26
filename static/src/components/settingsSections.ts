/** Stable section IDs also preserve shortcuts from other parts of the app. */
export type SettingsSection =
  | 'databases' | 'rigs' | 'sync'
  | 'setups' | 'templates'
  | 'calibration' | 'stacking'
  | 'performance' | 'gpu' | 'workers'
  | 'storage' | 'backups' | 'catalogs'
  | 'exports' | 'pixinsight' | 'astrobin'
  | 'review' | 'server' | 'users' | 'agents';

export interface SettingsPage {
  id: SettingsSection;
  label: string;
}

export interface SettingsGroup {
  id: string;
  label: string;
  pages: readonly SettingsPage[];
}

const GROUPS: readonly SettingsGroup[] = [
  { id: 'data', label: 'Databases & rigs', pages: [
    { id: 'databases', label: 'Databases' },
    { id: 'rigs', label: 'Rigs' },
    { id: 'sync', label: 'Sync' },
  ] },
  { id: 'presets', label: 'Presets', pages: [
    { id: 'setups', label: 'Processing setups' },
    { id: 'templates', label: 'Exposure templates' },
  ] },
  { id: 'processing', label: 'Processing', pages: [
    { id: 'calibration', label: 'Calibration' },
    { id: 'stacking', label: 'Stacking' },
  ] },
  { id: 'performance', label: 'Performance', pages: [
    { id: 'performance', label: 'CPU & memory' },
    { id: 'gpu', label: 'GPU' },
    { id: 'workers', label: 'Workers' },
  ] },
  { id: 'storage', label: 'Storage & backups', pages: [
    { id: 'storage', label: 'Folders & limits' },
    { id: 'backups', label: 'Backups' },
    { id: 'catalogs', label: 'Reference catalogs' },
  ] },
  { id: 'exports', label: 'Exports & integrations', pages: [
    { id: 'exports', label: 'Export defaults' },
    { id: 'pixinsight', label: 'PixInsight' },
    { id: 'astrobin', label: 'AstroBin' },
  ] },
  { id: 'review', label: 'Review', pages: [
    { id: 'review', label: 'Grading & display' },
  ] },
  { id: 'access', label: 'Server & access', pages: [
    { id: 'server', label: 'Server' },
    { id: 'users', label: 'Users' },
    { id: 'agents', label: 'Agents (MCP)' },
  ] },
];

export function settingsGroups(available: readonly SettingsSection[]): SettingsGroup[] {
  const allowed = new Set(available);
  return GROUPS.map(group => ({ ...group, pages: group.pages.filter(page => allowed.has(page.id)) }))
    .filter(group => group.pages.length > 0);
}

export function settingsPage(id: SettingsSection): SettingsPage {
  return GROUPS.flatMap(group => group.pages).find(page => page.id === id)!;
}

export function isSettingsSection(value: unknown): value is SettingsSection {
  return GROUPS.some(group => group.pages.some(page => page.id === value));
}
