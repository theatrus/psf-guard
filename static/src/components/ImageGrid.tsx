import { useState, useCallback, useRef, useEffect } from 'react';
import { useQuery, useMutation, useQueryClient } from '@tanstack/react-query';
import { useHotkeys } from 'react-hotkeys-hook';
import { useInView } from 'react-intersection-observer';
import { apiClient } from '../api/client';
import type { UpdateGradeRequest } from '../api/types';
import ImageCard from './ImageCard';
import ImageDetailView from './ImageDetailView';
import ImageComparisonView from './ImageComparisonView';

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
  const [showComparison, setShowComparison] = useState(false);
  const [comparisonRightId, setComparisonRightId] = useState<number | null>(null);
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

  // Update selected image when index changes or images load
  useEffect(() => {
    if (images[selectedIndex]) {
      setSelectedImageId(images[selectedIndex].id);
    } else if (images.length > 0 && selectedImageId === null) {
      // Auto-select first image if none selected
      setSelectedImageId(images[0].id);
      setSelectedIndex(0);
    }
  }, [selectedIndex, images, selectedImageId]);

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

  // Handle grading in comparison view
  const gradeComparisonImage = useCallback((imageId: number, status: 'accepted' | 'rejected' | 'pending') => {
    gradeMutation.mutate({
      imageId,
      request: { status },
    });
  }, [gradeMutation]);

  // Select right image for comparison
  const selectRightImage = useCallback(() => {
    console.log('selectRightImage called:', {
      selectedIndex,
      imagesLength: images.length,
      nextIndex: selectedIndex + 1
    });
    // Find next image after selected
    const nextIndex = selectedIndex + 1;
    if (nextIndex < images.length) {
      console.log('Setting comparison right ID to:', images[nextIndex].id);
      setComparisonRightId(images[nextIndex].id);
    } else {
      console.log('No next image available for comparison');
    }
  }, [selectedIndex, images]);

  // Debug: Log state changes
  useEffect(() => {
    console.log('ImageGrid state:', {
      selectedImageId,
      selectedIndex,
      showDetail,
      showComparison,
      comparisonRightId,
      imagesLength: images.length
    });
  }, [selectedImageId, selectedIndex, showDetail, showComparison, comparisonRightId, images.length]);

  // Keyboard shortcuts
  useHotkeys('k,right', () => {
    console.log('K/Right key pressed');
    navigateImages('next');
  }, [navigateImages]);
  useHotkeys('j,left', () => {
    console.log('J/Left key pressed');
    navigateImages('prev');
  }, [navigateImages]);
  useHotkeys('a', () => gradeImage('accepted'), [gradeImage]);
  useHotkeys('r', () => gradeImage('rejected'), [gradeImage]);
  useHotkeys('u', () => gradeImage('pending'), [gradeImage]);
  useHotkeys('enter', () => {
    console.log('Enter key pressed, opening detail view');
    setShowDetail(true);
  }, []);
  useHotkeys('escape', () => {
    console.log('Escape key pressed');
    if (showComparison) {
      setShowComparison(false);
      setComparisonRightId(null);
    } else {
      setShowDetail(false);
    }
  }, [showComparison]);
  useHotkeys('c', () => {
    console.log('C key pressed - checking conditions:', {
      selectedImageId,
      showDetail,
      showComparison,
      condition: selectedImageId && !showDetail && !showComparison
    });
    if (selectedImageId && !showDetail && !showComparison) {
      console.log('Opening comparison view!');
      setShowComparison(true);
      selectRightImage();
    }
  }, { enabled: !showDetail && !showComparison }, [selectedImageId, selectRightImage, showDetail, showComparison]);

  if (isLoading && page === 0) {
    return <div className="loading">Loading images...</div>;
  }

  return (
    <>
      {images.length > 0 && (
        <div className="grid-toolbar">
          <div className="toolbar-info">
            {images.length} images loaded
            {selectedImageId ? ` • Selected: Image #${selectedImageId}` : ' • Click an image to select it'}
          </div>
          <div className="toolbar-actions">
            <button 
              className="toolbar-button"
              onClick={() => {
                console.log('Compare button clicked:', {
                  selectedImageId,
                  showDetail,
                  showComparison,
                  imagesLength: images.length
                });
                if (selectedImageId) {
                  console.log('Opening comparison from button!');
                  setShowComparison(true);
                  selectRightImage();
                } else {
                  console.log('No image selected!');
                }
              }}
              disabled={!selectedImageId}
              title="Compare images side-by-side (C)"
            >
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                <rect x="2" y="4" width="9" height="16" rx="1" />
                <rect x="13" y="4" width="9" height="16" rx="1" />
              </svg>
              Compare
            </button>
          </div>
        </div>
      )}
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

      {showDetail && selectedImageId && !showComparison && (
        <ImageDetailView
          imageId={selectedImageId}
          onClose={() => setShowDetail(false)}
          onNext={() => navigateImages('next')}
          onPrevious={() => navigateImages('prev')}
          onGrade={gradeImage}
        />
      )}

      {showComparison && selectedImageId && (
        <>
          {console.log('Rendering ImageComparisonView with:', {
            leftImageId: selectedImageId,
            rightImageId: comparisonRightId
          })}
          <ImageComparisonView
            leftImageId={selectedImageId}
            rightImageId={comparisonRightId}
            onClose={() => {
              console.log('Closing comparison view');
              setShowComparison(false);
              setComparisonRightId(null);
            }}
            onSelectRightImage={selectRightImage}
            onGradeLeft={(status) => gradeComparisonImage(selectedImageId, status)}
            onGradeRight={(status) => comparisonRightId && gradeComparisonImage(comparisonRightId, status)}
          />
        </>
      )}
    </>
  );
}