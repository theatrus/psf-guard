import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, useLocation } from 'react-router-dom';
import ScoringChangedNote from '../ScoringChangedNote';
import { SCORING_DEFAULTS as DEFAULTS, setScoringDefaults } from '../../hooks/useScoringPreferences';

let search = '';
function LocationProbe() {
  search = useLocation().search;
  return null;
}

function renderNote(query: string) {
  return render(
    <MemoryRouter initialEntries={[`/grid?${query}`]}>
      <ScoringChangedNote />
      <LocationProbe />
    </MemoryRouter>,
  );
}

describe('the note on views that follow Sequence\'s scoring', () => {
  beforeEach(() => setScoringDefaults({ ...DEFAULTS }));
  afterEach(() => setScoringDefaults({ ...DEFAULTS }));

  it('says nothing while the view scores as the defaults do', () => {
    const view = renderNote('db=main&project=1');
    expect(view.container).toBeEmptyDOMElement();
    // A link naming the defaults is no change either.
    view.unmount();
    expect(renderNote('db=main&project=1&penaltySatellite=1&maxHfr=off').container)
      .toBeEmptyDOMElement();
  });

  it('says the view differs, and what, and resets it', async () => {
    const user = userEvent.setup();
    renderNote('db=main&project=1&penaltySatellite=0&penaltyPointing=1&penaltyTemporal=1&maxHfr=off&minStars=40');
    expect(screen.getByText(/^Scoring \(changed\)/)).toHaveAttribute(
      'title',
      'This view scores with its own values, not your defaults: '
        + 'Satellite trails off (default 100%), Min stars 40 (default off).',
    );
    await user.click(screen.getByRole('button', { name: 'Reset scoring to default' }));
    expect(search).toBe('?db=main&project=1');
    expect(screen.queryByText(/^Scoring \(changed\)/)).toBeNull();
  });
});
