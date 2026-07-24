import { useLocation, useParams, useNavigate, useSearchParams } from 'react-router-dom';
import { useDbProjectTarget } from '../hooks/useUrlState';
import GroupedImageGrid from './GroupedImageGrid';
import ImageDetailView from './ImageDetailView';
import ImageComparisonView from './ImageComparisonView';
import { useImageNavigation } from '../hooks/useImageNavigation';
import { useGrading } from '../hooks/useGrading';

export default function MainView() {
  const location = useLocation();
  const params = useParams();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const { dbId } = useDbProjectTarget();

  // Determine current view mode from URL
  const isDetailView = location.pathname.startsWith('/detail/');
  const isComparisonView = location.pathname.startsWith('/compare/');

  const imageId = params.imageId ? parseInt(params.imageId, 10) : undefined;
  const leftImageId = params.leftImageId ? parseInt(params.leftImageId, 10) : undefined;
  const rightImageId = params.rightImageId ? parseInt(params.rightImageId, 10) : undefined;

  // Navigation and grading hooks for overlays
  const navigation = useImageNavigation(imageId || rightImageId);
  const grading = useGrading(dbId!);

  const handleGrade = async (status: 'accepted' | 'rejected' | 'pending') => {
    if (!imageId) return;
    
    try {
      await grading.gradeImage(imageId, status);
      // Auto-advance to next image after grading
      if (navigation.canGoNext) {
        setTimeout(() => navigation.goToNext(), 100);
      }
    } catch (error) {
      console.error('Failed to grade image:', error);
    }
  };

  // Comparison view handlers
  const handleGradeLeft = async (status: 'accepted' | 'rejected' | 'pending') => {
    if (!leftImageId) return;
    
    try {
      await grading.gradeImage(leftImageId, status);
    } catch (error) {
      console.error('Failed to grade left image:', error);
    }
  };

  const handleGradeRight = async (status: 'accepted' | 'rejected' | 'pending') => {
    if (!rightImageId) return;
    
    try {
      await grading.gradeImage(rightImageId, status);
    } catch (error) {
      console.error('Failed to grade right image:', error);
    }
  };

  const handleNavigateRightNext = () => {
    if (!leftImageId) return;
    
    const currentIndex = navigation.allImages.findIndex(img => img.id === rightImageId);
    if (currentIndex >= 0) {
      // Find the next image that is different from the left image
      for (let i = currentIndex + 1; i < navigation.allImages.length; i++) {
        const nextRightImage = navigation.allImages[i];
        if (leftImageId !== nextRightImage.id) {
          const params = searchParams.toString();
          navigate(`/compare/${leftImageId}/${nextRightImage.id}?${params}`, { replace: true });
          return;
        }
      }
      // If we get here, no different image was found after current position
    }
  };

  const handleNavigateRightPrev = () => {
    if (!leftImageId) return;
    
    const currentIndex = navigation.allImages.findIndex(img => img.id === rightImageId);
    if (currentIndex >= 0) {
      // Find the previous image that is different from the left image
      for (let i = currentIndex - 1; i >= 0; i--) {
        const prevRightImage = navigation.allImages[i];
        if (leftImageId !== prevRightImage.id) {
          const params = searchParams.toString();
          navigate(`/compare/${leftImageId}/${prevRightImage.id}?${params}`, { replace: true });
          return;
        }
      }
      // If we get here, no different image was found before current position
    }
  };

  const handleSwapImages = () => {
    // Prevent swapping if both sides have the same image
    if (leftImageId !== rightImageId) {
      const params = searchParams.toString();
      navigate(`/compare/${rightImageId}/${leftImageId}?${params}`, { replace: true });
    }
  };

  const handleSelectRightImage = () => {
    // TODO: Implement image selection modal/dropdown
  };


  // Create adjacent image IDs for navigation context
  const adjacentImageIds = {
    next: navigation.canGoNext && navigation.currentIndex >= 0 ? 
      [navigation.allImages[navigation.currentIndex + 1]?.id].filter(Boolean) : [],
    previous: navigation.canGoPrevious && navigation.currentIndex >= 0 ? 
      [navigation.allImages[navigation.currentIndex - 1]?.id].filter(Boolean) : [],
  };

  // A DB with project=null means "All Projects" and is valid. No DB means
  // the user has not picked a project scope yet.
  if (!dbId) {
    const focusProjectPicker = () => {
      document.querySelector<HTMLSelectElement>('#project-select')?.focus();
    };

    return (
      <section className="project-pick-prompt" aria-labelledby="project-pick-title">
        <div className="project-pick-icon" aria-hidden="true">
          <span />
          <span />
          <span />
        </div>
        <div>
          <span className="project-pick-eyebrow">Images</span>
          <h2 id="project-pick-title">Choose a project</h2>
          <p>
            Pick a project from the menu above to browse, compare, and grade its frames.
          </p>
          <button type="button" onClick={focusProjectPicker}>
            Choose a project <span aria-hidden="true">↑</span>
          </button>
        </div>
      </section>
    );
  }

  return (
    <div className="main-view">
      {/* Always show the grid as the base layer */}
      <GroupedImageGrid useLazyImages={true} />

      {/* Show detail view overlay when in detail mode */}
      {isDetailView && imageId && dbId && (
        <div className="overlay-container detail-overlay">
          <ImageDetailView
            dbId={dbId}
            imageId={imageId}
            onClose={navigation.goToGrid}
            onNext={navigation.goToNext}
            onPrevious={navigation.goToPrevious}
            onGrade={handleGrade}
            adjacentImageIds={adjacentImageIds}
            grading={grading}
          />
        </div>
      )}

      {/* Show comparison view overlay when in comparison mode */}
      {isComparisonView && leftImageId && rightImageId && dbId && (
        <div className="overlay-container comparison-overlay">
          <ImageComparisonView
            dbId={dbId}
            leftImageId={leftImageId}
            rightImageId={rightImageId}
            onClose={navigation.goToGrid}
            onSelectRightImage={handleSelectRightImage}
            onGradeLeft={handleGradeLeft}
            onGradeRight={handleGradeRight}
            onNavigateRightNext={handleNavigateRightNext}
            onNavigateRightPrev={handleNavigateRightPrev}
            onSwapImages={handleSwapImages}
          />
        </div>
      )}
    </div>
  );
}
