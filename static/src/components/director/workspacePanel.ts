export function workspacePanel(id: string, tab: string, current: string) {
  return { role: 'tabpanel' as const, id: `${id}-panel-${tab}`, 'aria-labelledby': `${id}-tab-${tab}`, hidden: tab !== current, className: 'workspace-panel' };
}
