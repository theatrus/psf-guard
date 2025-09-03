import { useState, useEffect } from 'react';
import { apiClient } from '../api/client';
import type { PreviewOptions } from '../api/types';

export const useImageUrls = (imageId: number) => {
  const [previewUrl, setPreviewUrl] = useState<string>('');
  const [annotatedUrl, setAnnotatedUrl] = useState<string>('');
  const [psfUrl, setPsfUrl] = useState<string>('');

  useEffect(() => {
    const loadUrls = async () => {
      try {
        const [preview, annotated, psf] = await Promise.all([
          apiClient.getPreviewUrl(imageId, { size: 'screen', stretch: true }),
          apiClient.getAnnotatedUrl(imageId, 'large'),
          apiClient.getPsfUrl(imageId)
        ]);
        
        setPreviewUrl(preview);
        setAnnotatedUrl(annotated);
        setPsfUrl(psf);
      } catch (error) {
        console.error('Failed to load image URLs:', error);
      }
    };

    loadUrls();
  }, [imageId]);

  const getPreviewUrl = async (options?: PreviewOptions) => {
    return await apiClient.getPreviewUrl(imageId, options);
  };

  const getAnnotatedUrl = async (size: 'screen' | 'large' | 'original' = 'large', maxStars?: number) => {
    return await apiClient.getAnnotatedUrl(imageId, size, maxStars);
  };

  const getPsfUrl = async (options?: {
    num_stars?: number;
    psf_type?: string;
    sort_by?: string;
    grid_cols?: number;
    selection?: string;
  }) => {
    return await apiClient.getPsfUrl(imageId, options);
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