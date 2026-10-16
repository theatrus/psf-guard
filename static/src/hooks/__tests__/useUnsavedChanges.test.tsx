import { describe, expect, it, vi } from 'vitest';
import { renderHook } from '@testing-library/react';
import { describeUnsaved, unsavedLabels, useUnsavedChanges } from '../useUnsavedChanges';

describe('unsaved changes', () => {
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
