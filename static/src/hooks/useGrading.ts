import { useCallback } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { UpdateGradeRequest } from '../api/types';
import { useUndoRedo } from './useUndoRedo';
import { useAccess } from '../auth/access';

interface UseGradingOptions {
  onSuccess?: (imageIds: number[], status: string) => void;
  onError?: (error: Error, imageIds: number[]) => void;
}

export function useGrading(dbId: string, options: UseGradingOptions = {}) {
  const { onSuccess, onError } = options;
  const queryClient = useQueryClient();
  const undoRedo = useUndoRedo(dbId);
  const { canWrite } = useAccess();

  // One mutation serves any selection size: a single request and a single
  // server transaction. The response carries the grades it replaced, so
  // undo state costs no extra requests either — grading 2000 images was
  // previously ~4000 HTTP round trips (a state fetch and a write each).
  const batchGradeMutation = useMutation({
    mutationFn: async ({ imageIds, request, recordHistory = true }: {
      imageIds: number[];
      request: UpdateGradeRequest;
      recordHistory?: boolean;
    }) => {
      if (!canWrite) throw new Error('This account has read-only access');
      const response = await apiClient.batchUpdateImageGrades(
        dbId,
        imageIds.map((imageId) => ({
          image_id: imageId,
          status: request.status,
          reason: request.reason,
        }))
      );

      let actionId: string | null = null;
      if (recordHistory) {
        actionId = undoRedo.pushAction(
          imageIds,
          response.previous.map((entry) => ({
            imageId: entry.image_id,
            previousStatus: entry.status,
            previousReason: entry.reason ?? undefined,
          })),
          request.status,
          request.reason,
          `${request.status} ${imageIds.length === 1 ? 'image' : `${imageIds.length} images`}`
        );
      }

      return { imageIds, actionId };
    },
    onSuccess: (_, variables) => {
      // One prefix invalidation covers every per-image detail query.
      queryClient.invalidateQueries({ queryKey: ['db', dbId, 'image'] });
      queryClient.invalidateQueries({ queryKey: ['db', dbId, 'all-images'] });

      if (onSuccess) {
        onSuccess(variables.imageIds, variables.request.status);
      }
    },
    onError: (error: Error, variables) => {
      console.error('Batch grade failed:', error);
      if (onError) {
        onError(error, variables.imageIds);
      }
    },
  });

  // Convenience functions
  const gradeImage = useCallback((
    imageId: number,
    status: 'accepted' | 'rejected' | 'pending',
    reason?: string,
    recordHistory: boolean = true
  ) => {
    return batchGradeMutation.mutateAsync({
      imageIds: [imageId],
      request: { status, reason },
      recordHistory,
    });
  }, [batchGradeMutation]);

  const gradeBatch = useCallback((
    imageIds: number[], 
    status: 'accepted' | 'rejected' | 'pending',
    reason?: string,
    recordHistory: boolean = true
  ) => {
    return batchGradeMutation.mutateAsync({
      imageIds,
      request: { status, reason },
      recordHistory,
    });
  }, [batchGradeMutation]);

  // Auto-detection of single vs batch
  const gradeImages = useCallback((
    imageIds: number[], 
    status: 'accepted' | 'rejected' | 'pending',
    reason?: string,
    recordHistory: boolean = true
  ) => {
    if (imageIds.length === 1) {
      return gradeImage(imageIds[0], status, reason, recordHistory);
    } else {
      return gradeBatch(imageIds, status, reason, recordHistory);
    }
  }, [gradeImage, gradeBatch]);

  const isLoading = batchGradeMutation.isPending || undoRedo.isProcessing;
  const undo = useCallback(
    () => canWrite ? undoRedo.undo() : Promise.resolve(false),
    [canWrite, undoRedo],
  );
  const redo = useCallback(
    () => canWrite ? undoRedo.redo() : Promise.resolve(false),
    [canWrite, undoRedo],
  );

  return {
    // Grading functions
    gradeImage,
    gradeBatch, 
    gradeImages,
    
    // Undo/redo functions
    undo,
    redo,
    clearHistory: undoRedo.clearHistory,
    
    // State
    isLoading,
    canUndo: canWrite && undoRedo.canUndo,
    canRedo: canWrite && undoRedo.canRedo,
    canWrite,
    undoStackSize: undoRedo.undoStackSize,
    redoStackSize: undoRedo.redoStackSize,
    
    // Information
    getLastAction: undoRedo.getLastAction,
    getNextRedoAction: undoRedo.getNextRedoAction,
    
    // Raw mutation (for advanced usage)
    batchGradeMutation,
    
    // History (for debugging)
    undoStack: undoRedo.undoStack,
    redoStack: undoRedo.redoStack,
  };
}
