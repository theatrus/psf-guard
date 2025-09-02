import { useState, useCallback, useRef } from 'react';
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

export function useUndoRedo(options: UseUndoRedoOptions = {}) {
  const { maxHistorySize = 50 } = options;
  const queryClient = useQueryClient();
  
  const [undoStack, setUndoStack] = useState<GradingAction[]>([]);
  const [redoStack, setRedoStack] = useState<GradingAction[]>([]);
  const [isProcessing, setIsProcessing] = useState(false);
  
  // Keep track of current action ID to prevent duplicate tracking
  const currentActionId = useRef<string | null>(null);

  const generateActionId = () => {
    return Date.now().toString(36) + Math.random().toString(36).substr(2);
  };

  const getCurrentImageState = useCallback(async (imageId: number) => {
    try {
      const image = await apiClient.getImage(imageId);
      return {
        status: image.grading_status === 0 ? 'pending' as const :
                image.grading_status === 1 ? 'accepted' as const :
                'rejected' as const,
        reason: image.reject_reason || undefined,
      };
    } catch (error) {
      console.warn(`Failed to get current state for image ${imageId}:`, error);
      return null;
    }
  }, []);

  const recordAction = useCallback(async (
    imageIds: number[],
    newStatus: 'accepted' | 'rejected' | 'pending',
    newReason?: string,
    description?: string
  ) => {
    if (isProcessing) return null;

    const actionId = generateActionId();
    currentActionId.current = actionId;

    try {
      // Get previous states for all images
      const previousStatesPromises = imageIds.map(async (imageId) => {
        const currentState = await getCurrentImageState(imageId);
        if (!currentState) return null;
        
        return {
          imageId,
          previousStatus: currentState.status,
          previousReason: currentState.reason,
        };
      });

      const previousStates = (await Promise.all(previousStatesPromises))
        .filter((state): state is NonNullable<typeof state> => state !== null);

      if (previousStates.length === 0) {
        console.warn('No previous states found, skipping action recording');
        return null;
      }

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

      // Add to undo stack and clear redo stack
      setUndoStack(prev => {
        const newStack = [...prev, action];
        // Limit stack size
        if (newStack.length > maxHistorySize) {
          return newStack.slice(-maxHistorySize);
        }
        return newStack;
      });

      setRedoStack([]); // Clear redo stack when new action is recorded

      return actionId;
    } catch (error) {
      console.error('Failed to record action:', error);
      return null;
    } finally {
      currentActionId.current = null;
    }
  }, [isProcessing, maxHistorySize, getCurrentImageState]);

  const undo = useCallback(async () => {
    if (undoStack.length === 0 || isProcessing) return false;

    setIsProcessing(true);
    
    try {
      const action = undoStack[undoStack.length - 1];
      
      // Restore previous states for all images in the action
      const promises = action.previousStates.map(({ imageId, previousStatus, previousReason }) =>
        apiClient.updateImageGrade(imageId, {
          status: previousStatus,
          reason: previousReason,
        })
      );

      await Promise.all(promises);

      // Update stacks
      setUndoStack(prev => prev.slice(0, -1));
      setRedoStack(prev => [action, ...prev]);

      // Invalidate queries for affected images
      action.imageIds.forEach(imageId => {
        queryClient.invalidateQueries({ queryKey: ['image', imageId] });
      });
      queryClient.invalidateQueries({ queryKey: ['all-images'] });

      return true;
    } catch (error) {
      console.error('Undo failed:', error);
      return false;
    } finally {
      setIsProcessing(false);
    }
  }, [undoStack, isProcessing, queryClient]);

  const redo = useCallback(async () => {
    if (redoStack.length === 0 || isProcessing) return false;

    setIsProcessing(true);
    
    try {
      const action = redoStack[0];
      
      // Reapply the action to all images
      const promises = action.imageIds.map(imageId =>
        apiClient.updateImageGrade(imageId, {
          status: action.newStatus,
          reason: action.newReason,
        })
      );

      await Promise.all(promises);

      // Update stacks
      setRedoStack(prev => prev.slice(1));
      setUndoStack(prev => [...prev, action]);

      // Invalidate queries for affected images
      action.imageIds.forEach(imageId => {
        queryClient.invalidateQueries({ queryKey: ['image', imageId] });
      });
      queryClient.invalidateQueries({ queryKey: ['all-images'] });

      return true;
    } catch (error) {
      console.error('Redo failed:', error);
      return false;
    } finally {
      setIsProcessing(false);
    }
  }, [redoStack, isProcessing, queryClient]);

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
    recordAction,
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