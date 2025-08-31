import { useParams, useNavigate, useSearchParams } from 'react-router-dom';
import ImageComparisonView from './ImageComparisonView';
import { useImageNavigation } from '../hooks/useImageNavigation';
import { useGrading } from '../hooks/useGrading';

export default function ComparisonView() {
  const { leftImageId, rightImageId } = useParams<{ leftImageId: string; rightImageId: string }>();
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  
  const leftImageIdNum = leftImageId ? parseInt(leftImageId, 10) : undefined;
  const rightImageIdNum = rightImageId ? parseInt(rightImageId, 10) : undefined;
  
  const navigation = useImageNavigation(rightImageIdNum);
  const grading = useGrading();

  const handleClose = () => {
    // Preserve all current URL parameters when going back to grid
    navigate(`/grid?${searchParams.toString()}`, { replace: true });
  };

  const handleGradeLeft = async (status: 'accepted' | 'rejected' | 'pending') => {
    if (!leftImageIdNum) return;
    
    try {
      await grading.gradeImage(leftImageIdNum, status);
    } catch (error) {
      console.error('Failed to grade left image:', error);
    }
  };

  const handleGradeRight = async (status: 'accepted' | 'rejected' | 'pending') => {
    if (!rightImageIdNum) return;
    
    try {
      await grading.gradeImage(rightImageIdNum, status);
    } catch (error) {
      console.error('Failed to grade right image:', error);
    }
  };

  const handleNavigateRightNext = () => {
    if (!leftImageIdNum || !navigation.canGoNext) return;
    
    const currentIndex = navigation.allImages.findIndex(img => img.id === rightImageIdNum);
    if (currentIndex >= 0 && currentIndex < navigation.allImages.length - 1) {
      const nextRightImage = navigation.allImages[currentIndex + 1];
      const params = searchParams.toString();
      navigate(`/compare/${leftImageIdNum}/${nextRightImage.id}?${params}`, { replace: true });
    }
  };

  const handleNavigateRightPrev = () => {
    if (!leftImageIdNum || !navigation.canGoPrevious) return;
    
    const currentIndex = navigation.allImages.findIndex(img => img.id === rightImageIdNum);
    if (currentIndex > 0) {
      const prevRightImage = navigation.allImages[currentIndex - 1];
      const params = searchParams.toString();
      navigate(`/compare/${leftImageIdNum}/${prevRightImage.id}?${params}`, { replace: true });
    }
  };

  const handleSwapImages = () => {
    const params = searchParams.toString();
    navigate(`/compare/${rightImageId}/${leftImageId}?${params}`, { replace: true });
  };

  const handleSelectRightImage = () => {
    // TODO: Implement image selection modal/dropdown
    console.log('Image selection not yet implemented');
  };

  if (!leftImageId || !rightImageId) {
    return <div>Loading...</div>;
  }

  return (
    <ImageComparisonView
      leftImageId={leftImageIdNum!}
      rightImageId={rightImageIdNum!}
      onClose={handleClose}
      onSelectRightImage={handleSelectRightImage}
      onGradeLeft={handleGradeLeft}
      onGradeRight={handleGradeRight}
      onNavigateRightNext={handleNavigateRightNext}
      onNavigateRightPrev={handleNavigateRightPrev}
      onSwapImages={handleSwapImages}
    />
  );
}