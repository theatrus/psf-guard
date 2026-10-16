import { NIGHT_SPLIT_LABELS, useNightSplit, type NightSplit } from '../hooks/useNightSplit';

const SPLITS: readonly NightSplit[] = ['rig', 'catalog'];

/**
 * The View panel's choice of where this view splits lights into nights.
 * The server's `light_nights` setting chooses for everyone (Settings ›
 * Calibration); a choice here shows the other in this view only, through
 * its link, and the setting's own choice clears it.
 */
export default function NightSplitChoice({ id }: { id: string }) {
  const { split, setting, override, setSplit } = useNightSplit();
  return (
    <div className="toolbar-field">
      <label className="toolbar-caption" htmlFor={id}>Nights end</label>
      <select
        id={id}
        className={override && override !== setting ? 'is-set' : undefined}
        value={split}
        title="Where this view splits lights into nights. The Calibration setting chooses for every view; this shows the other here."
        onChange={(event) => setSplit(event.target.value === 'catalog' ? 'catalog' : 'rig')}
      >
        {SPLITS.map((choice) => (
          <option key={choice} value={choice}>
            {NIGHT_SPLIT_LABELS[choice]}{choice === setting ? ' (setting)' : ''}
          </option>
        ))}
      </select>
    </div>
  );
}

