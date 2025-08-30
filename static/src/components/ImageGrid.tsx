import { useState, useCallback, useRef, useEffect } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { useHotkeys } from 'react-hotkeys-hook';
import { useInView } from 'react-intersection-observer';
import { apiClient } from '../api/client';
import type { UpdateGradeRequest } from '../api/types';
import ImageCard from './ImageCard';
import ImageDetailView from './ImageDetailView';

interface ImageGridProps {
  projectId: number;
  targetId: number | null;
}

const ITEMS_PER_PAGE = 50;

export default function ImageGrid({ projectId, targetId }: ImageGridProps) {
  const queryClient = useQueryClient();
  const [selectedIndex, setSelectedIndex] = useState(0);
  const [selectedImageId, setSelectedImageId] = useState<number | null>(null);
  const [showDetail, setShowDetail] = useState(false);
  const [page, setPage] = useState(0);
  const gridRef = useRef<HTMLDivElement>(null);

  // Load more trigger
  const { ref: loadMoreRef, inView } = useInView({
    threshold: 0,
    rootMargin: '100px',
  });

  // Fetch images
  const { data: images = [], isLoading, isFetching } = useQuery({
    queryKey: ['images', projectId, targetId, page],
    queryFn: () => apiClient.getImages({
      project_id: projectId,
      target_id: targetId || undefined,
      limit: ITEMS_PER_PAGE,
      offset: page * ITEMS_PER_PAGE,
    }),
  });

  // Grade mutation
  const gradeMutation = useMutation({
    mutationFn: ({ imageId, request }: { imageId: number; request: UpdateGradeRequest }) =>
      apiClient.updateImageGrade(imageId, request),
    onSuccess: (_data, variables) => {
      // Invalidate both the images list and the individual image queries
      queryClient.invalidateQueries({ queryKey: ['images'] });
      queryClient.invalidateQueries({ queryKey: ['image', variables.imageId] });
    },
  });

  // Load more when scrolled to bottom
  useEffect(() => {
    if (inView && !isFetching && images.length === ITEMS_PER_PAGE) {
      setPage(p => p + 1);
    }
  }, [inView, isFetching, images.length]);

  // Update selected image when index changes
  useEffect(() => {
    if (images[selectedIndex]) {
      setSelectedImageId(images[selectedIndex].id);
    }
  }, [selectedIndex, images]);

  const navigateImages = useCallback((direction: 'next' | 'prev') => {
    setSelectedIndex(current => {
      if (direction === 'next') {
        return Math.min(current + 1, images.length - 1);
      } else {
        return Math.max(current - 1, 0);
      }
    });
  }, [images.length]);

  const gradeImage = useCallback((status: 'accepted' | 'rejected' | 'pending') => {
    if (!selectedImageId) return;

    gradeMutation.mutate({
      imageId: selectedImageId,
      request: { status },
    });

    // Auto-advance to next image
    setTimeout(() => navigateImages('next'), 100);
  }, [selectedImageId, gradeMutation, navigateImages]);

  // Keyboard shortcuts
  useHotkeys('j', () => navigateImages('next'), [navigateImages]);
  useHotkeys('k', () => navigateImages('prev'), [navigateImages]);
  useHotkeys('a', () => gradeImage('accepted'), [gradeImage]);
  useHotkeys('r', () => gradeImage('rejected'), [gradeImage]);
  useHotkeys('u', () => gradeImage('pending'), [gradeImage]);
  useHotkeys('enter', () => setShowDetail(true), []);
  useHotkeys('escape', () => setShowDetail(false), []);

  if (isLoading && page === 0) {
    return <div className="loading">Loading images...</div>;
  }

  return (
    <>
      <div className="image-grid" ref={gridRef}>
        {images.map((image, index) => (
          <ImageCard
            key={image.id}
            image={image}
            isSelected={selectedIndex === index}
            onClick={() => {
              setSelectedIndex(index);
              setSelectedImageId(image.id);
            }}
            onDoubleClick={() => setShowDetail(true)}
          />
        ))}
        
        {images.length === 0 && (
          <div className="empty-state">No images found</div>
        )}
        
        <div ref={loadMoreRef} className="load-more">
          {isFetching && <div>Loading more...</div>}
        </div>
      </div>

      {showDetail && selectedImageId && (
        <ImageDetailView
          imageId={selectedImageId}
          onClose={() => setShowDetail(false)}
          onNext={() => navigateImages('next')}
          onPrevious={() => navigateImages('prev')}
          onGrade={gradeImage}
        />
      )}
    </>
  );
}