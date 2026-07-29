import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { http, HttpResponse } from 'msw';
import { MemoryRouter, useLocation } from 'react-router-dom';
import type { ReactNode } from 'react';
import { beforeEach, describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import { useImageNavigation } from '../useImageNavigation';

function createWrapper(initialRoute: string) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={[initialRoute]}>{children}</MemoryRouter>
      </QueryClientProvider>
    );
  };
}

function NavigationHarness() {
  const navigation = useImageNavigation(1);
  const location = useLocation();
  return (
    <>
      <button type="button" onClick={navigation.closeDetail}>Close</button>
      <output data-testid="location">{location.pathname}{location.search}</output>
    </>
  );
}

describe('useImageNavigation close destination', () => {
  beforeEach(() => {
    server.use(
      http.get('/api/db/:dbId/images', () =>
        HttpResponse.json({ success: true, data: [], error: null, status: 'ready' })
      ),
    );
  });

  it('returns a Sequence detail view to its saved session', async () => {
    const user = userEvent.setup();
    render(<NavigationHarness />, {
      wrapper: createWrapper(
        '/detail/1?db=test&project=1&target=1&current=1&returnTo=sequence'
      ),
    });

    await user.click(screen.getByRole('button', { name: 'Close' }));

    expect(screen.getByTestId('location')).toHaveTextContent(
      '/sequence?db=test&project=1&target=1&current=1'
    );
  });

  it('keeps Images as the fallback for ordinary detail links', async () => {
    const user = userEvent.setup();
    render(<NavigationHarness />, {
      wrapper: createWrapper('/detail/1?db=test&project=1&current=1'),
    });

    await user.click(screen.getByRole('button', { name: 'Close' }));

    expect(screen.getByTestId('location')).toHaveTextContent(
      '/grid?db=test&project=1&current=1'
    );
  });
});
