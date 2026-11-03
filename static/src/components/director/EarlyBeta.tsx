import './EarlyBeta.css';

export const DIRECTOR_PLUGIN_BETA = 'The N.I.N.A. Director plugin is an early beta. Target Scheduler with PSF Guard Sync is the main way to run a rig.';
export const COLLABORATION_BETA = 'Collaboration through a shared server is an early beta.';

/** Marks what is still an early beta: what only the N.I.N.A. Director plugin
 *  drives (pairing, program pull, check-in and live status) and collaboration.
 *  Planning itself is not beta. */
export default function EarlyBeta({ note = DIRECTOR_PLUGIN_BETA }: { note?: string }) {
  return <span className="early-beta" title={note}>Early beta</span>;
}
