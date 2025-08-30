import { useCallback } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { UpdateGradeRequest } from '../api/types';
import { useUndoRedo } from './useUndoRedo';

interface UseGradingOptions {
  onSuccess?: (imageIds: number[], status: string) => void;
  onError?: (error: Error, imageIds: number[]) => void;
}

export function useGrading(options: UseGradingOptions = {}) {
  const { onSuccess, onError } = options;
  const queryClient = useQueryClient();
  const undoRedo = useUndoRedo();

  // Single image grading mutation
  const singleGradeMutation = useMutation({
    mutationFn: async ({ imageId, request, recordHistory = true }: { 
      imageId: number; 
      request: UpdateGradeRequest;
      recordHistory?: boolean;
    }) => {
      // Record action before applying if history tracking is enabled
      let actionId: string | null = null;
      if (recordHistory) {
        actionId = await undoRedo.recordAction(
          [imageId], 
          request.status, 
          request.reason,
          `${request.status} image`
        );
      }

      // Apply the grading change
      await apiClient.updateImageGrade(imageId, request);
      
      return { imageId, actionId };
    },
    onSuccess: (_, variables) => {
      // Invalidate queries
      queryClient.invalidateQueries({ queryKey: ['image', variables.imageId] });
      queryClient.invalidateQueries({ queryKey: ['all-images'] });
      
      if (onSuccess) {
        onSuccess([variables.imageId], variables.request.status);
      }
    },
    onError: (error: Error, variables) => {
      console.error('Single grade failed:', error);
      if (onError) {
        onError(error, [variables.imageId]);
      }
    },
  });

  // Batch grading mutation
  const batchGradeMutation = useMutation({
    mutationFn: async ({ imageIds, request, recordHistory = true }: { 
      imageIds: number[]; 
      request: UpdateGradeRequest;
      recordHistory?: boolean;
    }) => {
      // Record action before applying if history tracking is enabled
      let actionId: string | null = null;
      if (recordHistory) {
        actionId = await undoRedo.recordAction(
          imageIds, 
          request.status, 
          request.reason,
          `${request.status} ${imageIds.length} images`
        );
      }

      // Apply the grading changes
      const promises = imageIds.map(imageId =>
        apiClient.updateImageGrade(imageId, request)
      );
      
      await Promise.all(promises);
      
      return { imageIds, actionId };
    },
    onSuccess: (_, variables) => {
      // Invalidate queries for all affected images
      variables.imageIds.forEach(imageId => {
        queryClient.invalidateQueries({ queryKey: ['image', imageId] });
      });
      queryClient.invalidateQueries({ queryKey: ['all-images'] });
      
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
    return singleGradeMutation.mutateAsync({
      imageId,
      request: { status, reason },
      recordHistory,
    });
  }, [singleGradeMutation]);

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

  const isLoading = singleGradeMutation.isPending || batchGradeMutation.isPending || undoRedo.isProcessing;

  return {
    // Grading functions
    gradeImage,
    gradeBatch, 
    gradeImages,
    
    // Undo/redo functions
    undo: undoRedo.undo,
    redo: undoRedo.redo,
    clearHistory: undoRedo.clearHistory,
    
    // State
    isLoading,
    canUndo: undoRedo.canUndo,
    canRedo: undoRedo.canRedo,
    undoStackSize: undoRedo.undoStackSize,
    redoStackSize: undoRedo.redoStackSize,
    
    // Information
    getLastAction: undoRedo.getLastAction,
    getNextRedoAction: undoRedo.getNextRedoAction,
    
    // Raw mutations (for advanced usage)
    singleGradeMutation,
    batchGradeMutation,
    
    // History (for debugging)
    undoStack: undoRedo.undoStack,
    redoStack: undoRedo.redoStack,
  };
}