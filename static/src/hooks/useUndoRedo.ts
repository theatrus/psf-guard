import { useState, useCallback } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';

export interface GradingAction {
  id: string;
  type: 'single' | 'batch';
  timestamp: number;
  description: string;
  imageIds: number[];
  previousStates: Array<{
    imageId: number;
    previousStatus: 'accepted' | 'rejected' | 'pending';
    previousReason?: string;
  }>;
  newStatus: 'accepted' | 'rejected' | 'pending';
  newReason?: string;
}

interface UseUndoRedoOptions {
  maxHistorySize?: number;
}

export function useUndoRedo(dbId: string, options: UseUndoRedoOptions = {}) {
  const { maxHistorySize = 50 } = options;
  const queryClient = useQueryClient();

  const [undoStack, setUndoStack] = useState<GradingAction[]>([]);
  const [redoStack, setRedoStack] = useState<GradingAction[]>([]);
  const [isProcessing, setIsProcessing] = useState(false);

  const generateActionId = () => {
    return Date.now().toString(36) + Math.random().toString(36).substr(2);
  };

  const invalidateGrades = useCallback(() => {
    // One prefix invalidation covers every per-image detail query; a loop
    // of per-id invalidations over a large selection is pure overhead.
    queryClient.invalidateQueries({ queryKey: ['db', dbId, 'image'] });
    queryClient.invalidateQueries({ queryKey: ['db', dbId, 'all-images'] });
  }, [dbId, queryClient]);

  /** Record an applied action. The caller supplies the previous grades —
   * the batch grade endpoint returns them — so recording costs no
   * requests. */
  const pushAction = useCallback((
    imageIds: number[],
    previousStates: GradingAction['previousStates'],
    newStatus: 'accepted' | 'rejected' | 'pending',
    newReason?: string,
    description?: string
  ) => {
    if (previousStates.length === 0) return null;
    const actionId = generateActionId();
    const action: GradingAction = {
      id: actionId,
      type: imageIds.length > 1 ? 'batch' : 'single',
      timestamp: Date.now(),
      description: description || (
        imageIds.length > 1
          ? `${newStatus} ${imageIds.length} images`
          : `${newStatus} 1 image`
      ),
      imageIds,
      previousStates,
      newStatus,
      newReason,
    };

    setUndoStack(prev => {
      const newStack = [...prev, action];
      if (newStack.length > maxHistorySize) {
        return newStack.slice(-maxHistorySize);
      }
      return newStack;
    });
    setRedoStack([]); // Clear redo stack when new action is recorded

    return actionId;
  }, [maxHistorySize]);

  const undo = useCallback(async () => {
    if (undoStack.length === 0 || isProcessing) return false;

    setIsProcessing(true);

    try {
      const action = undoStack[undoStack.length - 1];

      // One request restores the whole action, mixed statuses included.
      await apiClient.batchUpdateImageGrades(
        dbId,
        action.previousStates.map(({ imageId, previousStatus, previousReason }) => ({
          image_id: imageId,
          status: previousStatus,
          reason: previousReason,
        }))
      );

      setUndoStack((prev) => prev.slice(0, -1));
      setRedoStack((prev) => [action, ...prev]);
      invalidateGrades();

      return true;
    } catch (error) {
      console.error('Undo failed:', error);
      return false;
    } finally {
      setIsProcessing(false);
    }
  }, [dbId, undoStack, isProcessing, invalidateGrades]);

  const redo = useCallback(async () => {
    if (redoStack.length === 0 || isProcessing) return false;

    setIsProcessing(true);

    try {
      const action = redoStack[0];

      await apiClient.batchUpdateImageGrades(
        dbId,
        action.imageIds.map((imageId) => ({
          image_id: imageId,
          status: action.newStatus,
          reason: action.newReason,
        }))
      );

      setRedoStack((prev) => prev.slice(1));
      setUndoStack((prev) => [...prev, action]);
      invalidateGrades();

      return true;
    } catch (error) {
      console.error('Redo failed:', error);
      return false;
    } finally {
      setIsProcessing(false);
    }
  }, [dbId, redoStack, isProcessing, invalidateGrades]);

  const clearHistory = useCallback(() => {
    setUndoStack([]);
    setRedoStack([]);
  }, []);

  const canUndo = undoStack.length > 0 && !isProcessing;
  const canRedo = redoStack.length > 0 && !isProcessing;

  const getLastAction = useCallback(() => {
    return undoStack[undoStack.length - 1] || null;
  }, [undoStack]);

  const getNextRedoAction = useCallback(() => {
    return redoStack[0] || null;
  }, [redoStack]);

  return {
    // Actions
    pushAction,
    undo,
    redo,
    clearHistory,

    // State
    canUndo,
    canRedo,
    isProcessing,
    undoStackSize: undoStack.length,
    redoStackSize: redoStack.length,

    // Getters
    getLastAction,
    getNextRedoAction,

    // History (for debugging/display)
    undoStack: [...undoStack], // Return copy to prevent external mutation
    redoStack: [...redoStack],
  };
}
