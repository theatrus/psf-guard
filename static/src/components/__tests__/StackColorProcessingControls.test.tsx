import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactElement } from 'react';
import { describe, expect, it, vi } from 'vitest';
import StackColorProcessingControls from '../StackColorProcessingControls';
import { defaultColorProcessing } from '../stackColorProcessing';

// The setups bar inside the editor reads the global setups list.
function renderWithQueries(element: ReactElement) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryClientProvider client={queryClient}>{element}</QueryClientProvider>
  );
}

describe('StackColorProcessingControls', () => {
  it('lets the user disable catalog background protection', async () => {
    const user = userEvent.setup();
    const onApply = vi.fn();
    renderWithQueries(
      <StackColorProcessingControls
        label="Test RGB"
        roles={['red', 'green', 'blue']}
        applied={defaultColorProcessing(['red', 'green', 'blue'])}
        backgrounds={{}}
        protections={{}}
        fallbacks={{}}
        deconvolutions={{}}
        disabled={false}
        onApply={onApply}
      />
    );

    await user.click(screen.getByText('Processing stack'));
    const protection = screen.getByRole('checkbox', {
      name: 'Protect catalog emission from background fitting',
    });
    expect(protection).toBeChecked();

    await user.click(protection);
    await user.click(screen.getByRole('button', { name: 'Apply processing stack' }));

    expect(onApply).toHaveBeenCalledWith(expect.objectContaining({
      background_extraction: expect.objectContaining({
        protect_catalog_emission: false,
      }),
    }));
  });
});
