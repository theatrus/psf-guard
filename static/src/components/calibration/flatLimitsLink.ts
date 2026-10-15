import type { FlatGroupsRig } from '../../api/types';
import { openSettings } from '../../utils/settingsIntent';

/** The flat-group setting a rig's Edit limits lands on: Group flats while
 *  it groups by time, else its dust limit, or the limit it sets itself. */
function limitsField(rig: FlatGroupsRig): string {
  if (!rig.measured) return 'flat_grouping';
  const own = (rig.own_limits ?? []).filter((key) => key !== 'flat_grouping');
  return own.length === 0 || own.includes('flat_split_dust_pct') ? 'flat_split_dust_pct' : own[0];
}

/**
 * Settings › Calibration, on the setting that decides how this rig's flats
 * split: the rig's own when it sets flat-group values of its own, else the
 * global one it inherits.
 */
export function editFlatLimits(rig: FlatGroupsRig): void {
  const field = limitsField(rig);
  const own = (rig.own_limits ?? []).includes(field);
  openSettings('calibration', own ? { rig: { uuid: rig.rig_uuid, name: rig.name }, field } : { field });
}
