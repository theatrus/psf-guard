import { afterEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import UndoRedoToolbar from '../UndoRedoToolbar';
import type { GradingAction } from '../../hooks/useUndoRedo';

const action = { description: 'Accepted 3 images' } as GradingAction;

function toolbar() {
  return (
    <UndoRedoToolbar
      canUndo
      canRedo={false}
      isProcessing={false}
      undoStackSize={1}
      redoStackSize={0}
      onUndo={async () => true}
      onRedo={async () => false}
      getLastAction={() => action}
      getNextRedoAction={() => null}
    />
  );
}

describe('UndoRedoToolbar', () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it('shows what was undone for two seconds', async () => {
    vi.useFakeTimers();
    render(toolbar());
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Undo/ }));
    });
    expect(screen.getByText('Undid: Accepted 3 images')).toBeInTheDocument();
    act(() => {
      vi.advanceTimersByTime(1999);
    });
    expect(screen.getByText('Undid: Accepted 3 images')).toBeInTheDocument();
    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(screen.queryByText('Undid: Accepted 3 images')).toBeNull();
  });

  it('stops its feedback timer when it unmounts', async () => {
    vi.useFakeTimers();
    const { unmount } = render(toolbar());
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Undo/ }));
    });
    expect(vi.getTimerCount()).toBeGreaterThan(0);
    // A toolbar that unmounts, as when a test or a view ends, leaves no timer
    // to update it after it is gone.
    unmount();
    expect(vi.getTimerCount()).toBe(0);
  });
});
