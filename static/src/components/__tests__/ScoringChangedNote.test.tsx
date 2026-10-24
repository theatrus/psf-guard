import { describe, expect, it } from 'vitest';
import { screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useLocation } from 'react-router-dom';
import ScoringChangedNote from '../ScoringChangedNote';
import { SCORING_DEFAULTS as DEFAULTS } from '../../hooks/useScoringPreferences';
import { renderWithScoring, scoringServer, waitForScoringDefaults } from '../../test/scoringDefaults';

let search = '';
function LocationProbe() {
  search = useLocation().search;
  return null;
}

function renderNote(query: string) {
  return renderWithScoring(
    <>
      <ScoringChangedNote />
      <LocationProbe />
    </>,
    { route: `/grid?${query}` },
  );
}

describe('the note on views that follow Sequence\'s scoring', () => {
  it('says nothing while the view scores as the defaults do', async () => {
    const view = renderNote('db=main&project=1');
    await waitForScoringDefaults(view.client);
    expect(view.container).toBeEmptyDOMElement();
    // A link naming the defaults is no change either.
    view.unmount();
    const named = renderNote('db=main&project=1&penaltySatellite=1&maxHfr=off');
    await waitForScoringDefaults(named.client);
    expect(named.container).toBeEmptyDOMElement();
  });

  it('compares the link with the server\'s defaults', async () => {
    scoringServer.set({ ...DEFAULTS, hfrRejectAbove: 3 });
    const view = renderNote('db=main&project=1&maxHfr=3');
    await waitForScoringDefaults(view.client);
    expect(view.container).toBeEmptyDOMElement();
  });

  it('says the view differs, and what, and resets it', async () => {
    const user = userEvent.setup();
    const view = renderNote('db=main&project=1&penaltySatellite=0&penaltyPointing=1&penaltyTemporal=1&maxHfr=off&minStars=40');
    await waitForScoringDefaults(view.client);
    expect(screen.getByText(/^Scoring \(changed\)/)).toHaveAttribute(
      'title',
      'This view scores with its own values, not the defaults: '
        + 'Satellite trails off (default 100%), Min stars 40 (default off).',
    );
    await user.click(screen.getByRole('button', { name: 'Reset scoring to default' }));
    expect(search).toBe('?db=main&project=1');
    expect(screen.queryByText(/^Scoring \(changed\)/)).toBeNull();
  });
});
