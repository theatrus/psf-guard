import { Crosshair } from 'lucide-react';
import type { PlacedRig } from './liveRigs';

const STATE_LABEL: Record<PlacedRig['connectivity'], string> = { online: 'Online', stale: 'Quiet', offline: 'Offline', never: 'Never seen' };

/** The fleet beside the map: each rig, whether it is talking, what it says
 *  it is doing, and where the map puts it. Choosing one turns the map to it. */
export default function SkyLive({ rigs, onFocus }: { rigs: PlacedRig[]; onFocus?: (rig: PlacedRig) => void }) {
  return <aside className="sky-live" aria-label="Live rigs on the sky">
    <h2>Live</h2>
    {rigs.length === 0 && <p className="sky-live-empty">No rig has reported yet.</p>}
    <ul>
      {rigs.map(rig => (
        <li key={rig.id} className={`sky-live-rig${rig.stale ? ' is-stale' : ''}${rig.exposing ? ' is-exposing' : ''}`}>
          <div className="sky-live-head">
            <strong>{rig.name}</strong>
            <span className={`sky-live-state is-${rig.connectivity}`}>{STATE_LABEL[rig.connectivity]}</span>
          </div>
          <div className="sky-live-now">{rig.now}{rig.staleReason === 'old report' ? ' · old report' : ''}</div>
          {rig.ra !== null && rig.dec !== null
            ? onFocus
              ? <button type="button" className="sky-live-focus" onClick={() => onFocus(rig)} aria-label={`Show ${rig.name} on the map`}>
                  <Crosshair size={13} aria-hidden="true" />{rig.source === 'pointing' ? 'Where it points' : `At ${rig.targetName} (its target)`}
                </button>
              : <span className="sky-live-unplaced">{rig.source === 'pointing' ? 'Pointing reported' : `At ${rig.targetName} (its target)`}</span>
            : <span className="sky-live-unplaced">{rig.unplaced}</span>}
        </li>
      ))}
    </ul>
  </aside>;
}
