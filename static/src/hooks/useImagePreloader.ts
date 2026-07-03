import { useEffect } from 'react';
import { apiClient } from '../api/client';
import { ensurePreviewReady } from './previewPoll';

/**
 * Hook to preload images for smooth navigation
 * @param currentImageId - The currently displayed image ID
 * @param nextImageIds - Array of image IDs that might be navigated to next
 * @param options - Preloading options
 */
export function useImagePreloader(
  dbId: string | null,
  currentImageId: number | null,
  nextImageIds: number[],
  options: {
    preloadCount?: number;
    includeAnnotated?: boolean;
    includeStarData?: boolean;
    imageSize?: 'screen' | 'large' | 'original';
  } = {}
) {
  const {
    preloadCount = 3,
    includeAnnotated = false,
    includeStarData = false,
    imageSize = 'large',
  } = options;

  useEffect(() => {
    if (!currentImageId || !dbId) return;

    // Preload the next N images. Warming goes through the interactive queue
    // (ensurePreviewReady), so an uncached preview is actually generated —
    // and its 202 no longer counts as a preload failure — by the time the
    // user navigates to it.
    const imagesToPreload = nextImageIds.slice(0, preloadCount);

    imagesToPreload.forEach((imageId) => {
      void ensurePreviewReady(
        dbId,
        apiClient.getPreviewUrl(dbId, imageId, { size: imageSize }),
        { imageId, kind: 'preview', size: imageSize }
      );

      // Optionally warm the annotated version (getAnnotatedUrl defaults to
      // 'large' with 1000 stars).
      if (includeAnnotated) {
        void ensurePreviewReady(
          dbId,
          apiClient.getAnnotatedUrl(dbId, imageId),
          { imageId, kind: 'annotated', size: 'large' }
        );
      }
    });

    // Optionally preload star detection data
    if (includeStarData) {
      imagesToPreload.forEach((imageId) => {
        // This will trigger the React Query cache
        apiClient.getStarDetection(dbId, imageId).catch(() => {
          // Ignore errors for preloading
        });
      });
    }

    return () => {
      // No cleanup needed for image preloading
    };
  }, [dbId, currentImageId, nextImageIds, preloadCount, includeAnnotated, includeStarData, imageSize]);
}

/**
 * Get the IDs of images that should be preloaded based on current navigation
 */
export function getNextImageIds(
  allImages: { id: number }[],
  currentImageId: number | null,
  direction: 'forward' | 'both' = 'both',
  count: number = 3
): number[] {
  if (!currentImageId || allImages.length === 0) return [];

  const currentIndex = allImages.findIndex(img => img.id === currentImageId);
  if (currentIndex === -1) return [];

  const nextIds: number[] = [];

  if (direction === 'forward' || direction === 'both') {
    // Add next images
    for (let i = 1; i <= count && currentIndex + i < allImages.length; i++) {
      nextIds.push(allImages[currentIndex + i].id);
    }
  }

  if (direction === 'both') {
    // Add previous images
    for (let i = 1; i <= count && currentIndex - i >= 0; i++) {
      nextIds.push(allImages[currentIndex - i].id);
    }
  }

  return nextIds;
}