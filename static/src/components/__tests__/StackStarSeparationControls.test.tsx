import { render, screen } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { expect, it, vi } from 'vitest';
import StackStarSeparationControls from '../StackStarSeparationControls';

it('identifies both linear star-separation files as downloads', () => {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  render(<QueryClientProvider client={client}>
    <StackStarSeparationControls label="Narrowband nebula" config={null} disabled={false} onChange={vi.fn()}
      result={{ cache_id: 'separated', processing: { steps: [] }, cli_version: '2.6.6', steps: [],
        has_stars: true, fits_url: '/nebula.fits', stars_fits_url: '/stars.fits' }} />
  </QueryClientProvider>);
  for (const [name, href] of [
    ['Download linear Narrowband nebula starless FITS', '/nebula.fits'],
    ['Download linear Narrowband nebula stars FITS', '/stars.fits'],
  ]) {
    const link = screen.getByRole('link', { name });
    expect(link).toHaveAttribute('href', href);
    expect(link).toHaveAttribute('download');
    expect(link.querySelector('svg')).toHaveAttribute('aria-hidden', 'true');
  }
});
