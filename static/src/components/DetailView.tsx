import { useParams } from 'react-router-dom';
import ImageDetailView from './ImageDetailView';
import { useImageNavigation } from '../hooks/useImageNavigation';
import { useGrading } from '../hooks/useGrading';
import { useDbProjectTarget } from '../hooks/useUrlState';

export default function DetailView() {
  const { imageId } = useParams<{ imageId: string }>();
  const imageIdNum = imageId ? parseInt(imageId, 10) : undefined;

  const { dbId } = useDbProjectTarget();
  const navigation = useImageNavigation(imageIdNum);
  const grading = useGrading(dbId!);

  const handleGrade = async (status: 'accepted' | 'rejected' | 'pending') => {
    if (!imageIdNum) return;

    try {
      await grading.gradeImage(imageIdNum, status);
      // Auto-advance to next image after grading
      if (navigation.canGoNext) {
        setTimeout(() => navigation.goToNext(), 100);
      }
    } catch (error) {
      console.error('Failed to grade image:', error);
    }
  };

  // Create adjacent image IDs for navigation context
  const adjacentImageIds = {
    next: navigation.canGoNext && navigation.currentIndex >= 0
      ? [navigation.allImages[navigation.currentIndex + 1]?.id].filter(Boolean)
      : [],
    previous: navigation.canGoPrevious && navigation.currentIndex >= 0
      ? [navigation.allImages[navigation.currentIndex - 1]?.id].filter(Boolean)
      : [],
  };

  if (!imageId) {
    return <div>Loading...</div>;
  }

  if (!dbId) {
    return (
      <div className="empty-state">
        Database not specified. <a href="#/overview">Back to overview</a>
      </div>
    );
  }

  return (
    <ImageDetailView
      dbId={dbId}
      imageId={imageIdNum!}
      onClose={navigation.goToGrid}
      onNext={navigation.goToNext}
      onPrevious={navigation.goToPrevious}
      onGrade={handleGrade}
      adjacentImageIds={adjacentImageIds}
      grading={grading}
    />
  );
}