import { useId } from 'react';
import { Link } from 'react-router-dom';
import { Map as MapIcon } from 'lucide-react';
import { useAccess } from '../../auth/access';
import { useUrlParams } from '../../hooks/useUrlState';
import { openSettings } from '../../utils/settingsIntent';
import { usePlans } from '../header/useCurrentPlan';
import DirectorDashboard from '../director/DirectorDashboard';
import DirectorRigs from '../director/DirectorRigs';
import TemplateLibrary from '../director/TemplateLibrary';
import WorkspaceTabs from '../director/WorkspaceTabs';
import NightsView from '../nights/NightsView';
import { NIGHTS_PARAMS, RIGS_TAB_PARAM } from '../nights/nightsModel';
import '../director/DirectorPage.css';
import './RigsPage.css';

type RigsTab = 'live' | 'nights';
const TABS = [
  { id: 'live', label: 'Live' },
  { id: 'nights', label: 'Nights' },
] as const satisfies readonly { id: RigsTab; label: string }[];

/**
 * Every rig in one place: **Live**, what each rig is doing now and plans
 * tonight, and **Nights**, what each would shoot over the coming nights,
 * with the nights marked clear or not. The header's rigs chip opens Live;
 * the Sky still draws each rig where it points.
 */
export default function RigsPage() {
  const id = useId();
  const { getParam, updateParams } = useUrlParams();
  const { canWrite } = useAccess();
  const plans = usePlans();
  const tab: RigsTab = getParam(RIGS_TAB_PARAM) === 'nights' ? 'nights' : 'live';
  if (!plans.enabled) {
    return <main className="director-page rigs-page"><p className="director-muted">Rigs need Director, which is off on this server.</p></main>;
  }
  const choose = (next: RigsTab) => updateParams({
    [RIGS_TAB_PARAM]: next === 'live' ? null : next,
    // A chosen night belongs to the Nights tab it was chosen on.
    [NIGHTS_PARAMS.rig]: null,
    [NIGHTS_PARAMS.night]: null,
  });
  return <main className="director-page rigs-page">
    <div className="rigs-head">
      <h1>Rigs</h1>
      <WorkspaceTabs id={id} label="Rigs" tabs={TABS} value={tab} onChange={choose} />
    </div>
    <div role="tabpanel" id={`${id}-panel-${tab}`} aria-labelledby={`${id}-tab-${tab}`}>
      {tab === 'live' ? <>
        <DirectorDashboard />
        <p className="director-muted">
          <Link to="/sky?live=1"><MapIcon size={14} aria-hidden="true" /> Show the rigs on the sky</Link>
          {canWrite && <>
            {' · '}Rig setup and the exposure template library are under Settings:{' '}
            <button type="button" className="director-link-button" onClick={() => openSettings('rigs')}>Rig setup</button>
            {' and '}
            <button type="button" className="director-link-button" onClick={() => openSettings('templates')}>Exposure templates</button>.
          </>}
        </p>
        {!canWrite && <><DirectorRigs /><TemplateLibrary /></>}
      </> : <NightsView />}
    </div>
  </main>;
}
