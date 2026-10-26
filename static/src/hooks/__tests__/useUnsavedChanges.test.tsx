import { describe, expect, it, vi } from 'vitest';
import { renderHook } from '@testing-library/react';
import { describeUnsaved, unsavedLabels, UnsavedScopeContext, useUnsavedChanges, useUnsavedLabels } from '../useUnsavedChanges';
import type { ReactNode } from 'react';

describe('unsaved changes', () => {
  it('scopes navigation warnings by form instance without changing page-unload warnings', () => {
    const scope = new Set<symbol>();
    const outside = renderHook(() => useUnsavedChanges('Rig profile', true));
    const inside = renderHook(({ dirty }) => {
      useUnsavedChanges('Rig profile', dirty);
      return useUnsavedLabels(scope);
    }, {
      initialProps: { dirty: false },
      wrapper: ({ children }: { children: ReactNode }) => <UnsavedScopeContext.Provider value={scope}>{children}</UnsavedScopeContext.Provider>,
    });
    expect(inside.result.current).toEqual([]);
    expect(unsavedLabels()).toEqual(['Rig profile']);
    inside.rerender({ dirty: true });
    expect(inside.result.current).toEqual(['Rig profile']);
    expect(unsavedLabels()).toEqual(['Rig profile', 'Rig profile']);
    inside.rerender({ dirty: false });
    expect(inside.result.current).toEqual([]);
    expect(unsavedLabels()).toEqual(['Rig profile']);
    inside.unmount();
    outside.unmount();
    expect(unsavedLabels()).toEqual([]);
  });

  it('registers a form while it is dirty, and the page asks before it goes', () => {
    const add = vi.spyOn(window, 'addEventListener');
    const remove = vi.spyOn(window, 'removeEventListener');
    const { rerender, unmount } = renderHook(({ dirty }) => useUnsavedChanges('C925 setup', dirty), {
      initialProps: { dirty: false },
    });
    expect(unsavedLabels()).toEqual([]);
    rerender({ dirty: true });
    expect(unsavedLabels()).toEqual(['C925 setup']);
    expect(add).toHaveBeenCalledWith('beforeunload', expect.any(Function));
    unmount();
    expect(unsavedLabels()).toEqual([]);
    expect(remove).toHaveBeenCalledWith('beforeunload', expect.any(Function));
    add.mockRestore();
    remove.mockRestore();
  });

  it('names one, two or several forms', () => {
    expect(describeUnsaved(['C925 setup'])).toBe('C925 setup');
    expect(describeUnsaved(['C925 setup', 'site Folsom'])).toBe('C925 setup and site Folsom');
    expect(describeUnsaved(['a', 'b', 'c'])).toBe('a, b and c');
  });
});
