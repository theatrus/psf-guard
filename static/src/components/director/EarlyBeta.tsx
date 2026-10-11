import './EarlyBeta.css';

export const EARLY_BETA_NOTE = 'Pairing with the N.I.N.A. Director plugin is an early beta. Target Scheduler with PSF Guard Sync is the main path today.';

/** Marks what only the N.I.N.A. Director plugin drives: pairing, program
 *  pull, check-in and live status. Planning itself is not beta. */
export default function EarlyBeta() {
  return <span className="early-beta" title={EARLY_BETA_NOTE}>Early beta</span>;
}
