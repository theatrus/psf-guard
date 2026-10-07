import { afterEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { server } from '../../../test/msw-server';
import StacksView from '../StacksView';

// The panel's own requests are its tests'; here it only shows the size it gets.
vi.mock('../../StackPreviewPanel', () => ({
  default: ({ cardSize }: { cardSize: number | null }) => <output data-testid="card-size">{cardSize === null ? 'fit' : cardSize}</output>,
}));
vi.mock('../MosaicStacks', () => ({ default: () => null }));
vi.mock('../WbppStacks', () => ({ default: () => null }));
vi.mock('../../ProjectExposureGrouping', () => ({ default: () => null }));

const KEY = 'psf-guard.stacks.card-size';
function Where() { return <output data-testid="where">{useLocation().search}</output>; }

function mount(route = '/stacks?db=test&project=1') {
  server.use(
    http.get('/api/db/test/images', () => HttpResponse.json({ success: true, data: [], error: null })),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[route]}>
        <StacksView />
        <Where />
      </MemoryRouter>
    </QueryClientProvider>
  );
}

describe('Stacks card size', () => {
  afterEach(() => window.localStorage.clear());

  it('fits the cards to the row until a size is chosen, and keeps the choice in this browser', async () => {
    const first = mount();
    expect(await screen.findByTestId('card-size')).toHaveTextContent('fit');
    expect(screen.getByText('Fit')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Fit' })).not.toBeInTheDocument();

    fireEvent.change(screen.getByLabelText('Card size:'), { target: { value: '700' } });
    expect(screen.getByTestId('card-size')).toHaveTextContent('700');
    expect(screen.getByTestId('where')).toHaveTextContent('cardsize=700');
    expect(window.localStorage.getItem(KEY)).toBe('700');
    first.unmount();

    // A fresh page in this browser, with no size in its link, keeps it.
    const second = mount();
    expect(await screen.findByTestId('card-size')).toHaveTextContent('700');
    // Fit goes back to filling the row, and forgets the choice.
    fireEvent.click(screen.getByRole('button', { name: 'Fit' }));
    expect(screen.getByTestId('card-size')).toHaveTextContent('fit');
    expect(window.localStorage.getItem(KEY)).toBeNull();
    expect(screen.getByTestId('where')).not.toHaveTextContent('cardsize');
    second.unmount();
  });

  it('takes a size in the link over the one this browser keeps', async () => {
    window.localStorage.setItem(KEY, '700');
    mount('/stacks?db=test&project=1&cardsize=400');
    expect(await screen.findByTestId('card-size')).toHaveTextContent('400');
  });
});
