import { useEffect } from 'react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, useLocation, type NavigateOptions, type To } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import type { Image, OrganizationResult } from '../../api/types';
import GroupedImageGrid from '../GroupedImageGrid';

const navigation = vi.hoisted(() => ({
  pending: null as (() => void | Promise<void>) | null,
  locations: [] as string[],
}));

vi.mock('react-router-dom', async (importOriginal) => {
  const actual = await importOriginal<typeof import('react-router-dom')>();
  return {
    ...actual,
    useNavigate: () => {
      const navigate = actual.useNavigate();
      return (to: To | number, options?: NavigateOptions) => {
        if (typeof to === 'string' && to.startsWith('/grid?')) {
          navigation.pending = () => navigate(to, options);
          return;
        }
        return typeof to === 'number' ? navigate(to) : navigate(to, options);
      };
    },
  };
});

vi.mock('../OrganizationDialog', () => ({
  default: ({ onApplied, onClose }: {
    onApplied: (result: OrganizationResult) => void;
    onClose: () => void;
  }) => (
    <div role="dialog" aria-label="Move exposures">
      <button type="button" onClick={() => {
        onApplied({ project_id: 2, target_id: 21, images_moved: 1 });
        onClose();
      }}>Apply move</button>
      <button type="button" onClick={onClose}>Cancel move</button>
    </div>
  ),
}));

vi.mock('../ImageCard', () => ({
  default: ({ image }: { image: Image }) => (
    <div
      className="image-card"
      data-testid={`image-${image.id}`}
      data-card-image-id={image.id}
      data-target-id={image.target_id}
    >
      {image.target_name}
    </div>
  ),
}));

vi.mock('../StackPreviewPanel', () => ({ default: () => null }));

const DB_ID = 'c925-review';
const sourceKey = ['db', DB_ID, 'all-images', 1, 11];
const sourceRoute = `/grid?db=${DB_ID}&project=1&target=11&current=1&selected=1`;

const firstImage: Image = {
  id: 1,
  project_id: 1,
  project_name: 'Source project',
  project_display_name: 'Source project',
  target_id: 11,
  target_name: 'Source target',
  acquired_date: 1_705_352_400,
  filter_name: 'R',
  grading_status: 1,
  reject_reason: null,
  metadata: { FileName: 'first.fits' },
  filesystem_path: '/images/first.fits',
};
const secondImage: Image = {
  ...firstImage,
  id: 2,
  acquired_date: firstImage.acquired_date! + 60,
  metadata: { FileName: 'second.fits' },
  filesystem_path: '/images/second.fits',
};
const movedImage: Image = {
  ...firstImage,
  project_id: 2,
  project_name: 'Destination project',
  project_display_name: 'Destination project',
  target_id: 21,
  target_name: 'Destination target',
};

function RouteProbe() {
  const location = useLocation();
  const route = `${location.pathname}${location.search}`;
  useEffect(() => { navigation.locations.push(route); }, [route]);
  return <output data-testid="route">{route}</output>;
}

function mountGrid() {
  let sourceImages = [firstImage, secondImage];
  server.use(
    http.get('/api/info', () => HttpResponse.json({
      success: true,
      data: { version: 'test', allow_database_management: true },
      error: null,
      status: 'ready',
    })),
    http.get('/api/db/:dbId/images', ({ request }) => HttpResponse.json({
      success: true,
      data: new URL(request.url).searchParams.get('target_id') === '21'
        ? [movedImage]
        : sourceImages,
      error: null,
      status: 'ready',
    })),
  );
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0, staleTime: Infinity } },
  });
  client.setQueryData(sourceKey, sourceImages);
  render(
    <QueryClientProvider client={client}>
      <MemoryRouter initialEntries={[sourceRoute]}>
        <RouteProbe />
        <GroupedImageGrid />
      </MemoryRouter>
    </QueryClientProvider>,
  );
  return {
    refreshSourceAfterMove: async () => {
      sourceImages = [secondImage];
      await act(async () => {
        await client.refetchQueries({ queryKey: sourceKey, exact: true });
      });
      await waitFor(() => expect(screen.queryByTestId('image-1')).not.toBeInTheDocument());
    },
  };
}

beforeEach(() => {
  navigation.pending = null;
  navigation.locations = [];
});

describe('GroupedImageGrid organization navigation', () => {
  it('does not normalize the source cursor during a move or while destination navigation is pending', async () => {
    const { refreshSourceAfterMove } = mountGrid();
    await screen.findByTestId('image-1');
    await screen.findByTestId('image-2');
    await userEvent.click(await screen.findByRole('button', { name: 'Move exposures' }));
    await screen.findByRole('dialog', { name: 'Move exposures' });

    // The source request can finish before the router commits the destination.
    await refreshSourceAfterMove();
    expect(screen.getByTestId('route')).toHaveTextContent(sourceRoute);
    await userEvent.click(screen.getByRole('button', { name: 'Apply move' }));
    expect(screen.queryByRole('dialog', { name: 'Move exposures' })).not.toBeInTheDocument();
    expect(navigation.pending).not.toBeNull();
    expect(screen.getByTestId('route')).toHaveTextContent(sourceRoute);
    expect(navigation.locations).toEqual([sourceRoute]);

    await act(async () => { await navigation.pending!(); });
    expect(await screen.findByTestId('image-1')).toHaveAttribute('data-target-id', '21');
    expect(screen.queryByTestId('image-2')).not.toBeInTheDocument();
    await waitFor(() => expect(screen.getByTestId('route')).toHaveTextContent(`project=2&target=21&current=1`));
    expect(navigation.locations.slice(1).every(route => new URLSearchParams(route.split('?')[1]).get('target') === '21'))
      .toBe(true);
  });

  it('resumes source cursor normalization when the move is canceled', async () => {
    const { refreshSourceAfterMove } = mountGrid();
    await userEvent.click(await screen.findByRole('button', { name: 'Move exposures' }));
    await refreshSourceAfterMove();
    expect(screen.getByTestId('route')).toHaveTextContent(sourceRoute);
    await userEvent.click(screen.getByRole('button', { name: 'Cancel move' }));
    await waitFor(() => expect(screen.getByTestId('route')).toHaveTextContent('target=11&current=2'));
    expect(navigation.pending).toBeNull();
    expect(screen.getByTestId('image-2')).toHaveAttribute('data-target-id', '11');
  });
});
