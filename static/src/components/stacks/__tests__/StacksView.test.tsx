import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter } from 'react-router-dom';
import StacksView from '../StacksView';

describe('StacksView', () => {
  it('asks for a project before it can stack anything', () => {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={queryClient}>
        <MemoryRouter initialEntries={['/stacks?db=test']}>
          <StacksView />
        </MemoryRouter>
      </QueryClientProvider>
    );
    expect(screen.getByRole('heading', { name: 'Stacks' })).toBeInTheDocument();
    expect(screen.getByText('Choose a project in the header to stack its frames.')).toBeInTheDocument();
  });
});
