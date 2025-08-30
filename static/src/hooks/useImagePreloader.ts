import { useEffect } from 'react';
import { apiClient } from '../api/client';

/**
 * Hook to preload images for smooth navigation
 * @param currentImageId - The currently displayed image ID
 * @param nextImageIds - Array of image IDs that might be navigated to next
 * @param options - Preloading options
 */
export function useImagePreloader(
  currentImageId: number | null,
  nextImageIds: number[],
  options: {
    preloadCount?: number;
    includeAnnotated?: boolean;
    includeStarData?: boolean;
    imageSize?: 'screen' | 'large';
  } = {}
) {
  const { 
    preloadCount = 3, 
    includeAnnotated = false,
    includeStarData = false,
    imageSize = 'large'
  } = options;

  useEffect(() => {
    if (!currentImageId) return;

    // Preload the next N images
    const imagesToPreload = nextImageIds.slice(0, preloadCount);
    const preloadPromises: Promise<void>[] = [];

    imagesToPreload.forEach(imageId => {
      // Preload the regular preview
      const previewUrl = apiClient.getPreviewUrl(imageId, { size: imageSize });
      const previewImg = new Image();
      previewImg.src = previewUrl;
      preloadPromises.push(
        new Promise((resolve) => {
          previewImg.onload = () => resolve();
          previewImg.onerror = () => resolve(); // Resolve even on error to not block
        })
      );

      // Optionally preload annotated version
      if (includeAnnotated) {
        const annotatedUrl = apiClient.getAnnotatedUrl(imageId);
        const annotatedImg = new Image();
        annotatedImg.src = annotatedUrl;
        preloadPromises.push(
          new Promise((resolve) => {
            annotatedImg.onload = () => resolve();
            annotatedImg.onerror = () => resolve();
          })
        );
      }
    });

    // Optionally preload star detection data
    if (includeStarData) {
      imagesToPreload.forEach(imageId => {
        // This will trigger the React Query cache
        apiClient.getStarDetection(imageId).catch(() => {
          // Ignore errors for preloading
        });
      });
    }

    return () => {
      // No cleanup needed for image preloading
    };
  }, [currentImageId, nextImageIds, preloadCount, includeAnnotated, includeStarData, imageSize]);
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