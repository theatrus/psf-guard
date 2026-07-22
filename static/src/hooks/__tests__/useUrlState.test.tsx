import type { PropsWithChildren } from 'react';
import { act, renderHook, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { useGridState } from '../useUrlState';

function Router({ children }: PropsWithChildren) {
  return <MemoryRouter initialEntries={['/grid']}>{children}</MemoryRouter>;
}

describe('useGridState', () => {
  it('keeps rapid functional multi-selection updates and a stable setter', async () => {
    const { result } = renderHook(() => useGridState(), { wrapper: Router });
    const initialSetter = result.current.setSelectedImages;

    act(() => {
      result.current.setCurrentImageSelection(11);
      result.current.setSelectedImages((selected) => new Set(selected).add(12));
    });

    await waitFor(() => {
      expect([...result.current.selectedImages]).toEqual([11, 12]);
      expect(result.current.currentImageId).toBe(11);
    });

    act(() => result.current.setCurrentImageId(12));
    await waitFor(() => expect(result.current.currentImageId).toBe(12));
    expect([...result.current.selectedImages]).toEqual([11, 12]);
    expect(result.current.setSelectedImages).toBe(initialSetter);
  });
});
