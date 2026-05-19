import { useState, useEffect } from 'react';
import { apiClient } from '../api/client';
import type { PreviewOptions } from '../api/types';

export const useImageUrls = (dbId: string, imageId: number) => {
  const [previewUrl, setPreviewUrl] = useState<string>('');
  const [annotatedUrl, setAnnotatedUrl] = useState<string>('');
  const [psfUrl, setPsfUrl] = useState<string>('');

  useEffect(() => {
    if (!dbId) return;
    const loadUrls = async () => {
      try {
        const [preview, annotated, psf] = await Promise.all([
          apiClient.getPreviewUrl(dbId, imageId, { size: 'screen', stretch: true }),
          apiClient.getAnnotatedUrl(dbId, imageId, 'large'),
          apiClient.getPsfUrl(dbId, imageId),
        ]);

        setPreviewUrl(preview);
        setAnnotatedUrl(annotated);
        setPsfUrl(psf);
      } catch (error) {
        console.error('Failed to load image URLs:', error);
      }
    };

    loadUrls();
  }, [dbId, imageId]);

  const getPreviewUrl = async (options?: PreviewOptions) => {
    return apiClient.getPreviewUrl(dbId, imageId, options);
  };

  const getAnnotatedUrl = async (
    size: 'screen' | 'large' | 'original' = 'large',
    maxStars?: number
  ) => {
    return apiClient.getAnnotatedUrl(dbId, imageId, size, maxStars);
  };

  const getPsfUrl = async (options?: {
    num_stars?: number;
    psf_type?: string;
    sort_by?: string;
    grid_cols?: number;
    selection?: string;
  }) => {
    return apiClient.getPsfUrl(dbId, imageId, options);
  };

  return {
    previewUrl,
    annotatedUrl,
    psfUrl,
    getPreviewUrl,
    getAnnotatedUrl,
    getPsfUrl,
  };
};