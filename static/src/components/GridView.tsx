import { useProjectTarget } from '../hooks/useUrlState';
import GroupedImageGrid from './GroupedImageGrid';

export default function GridView() {
  const { projectId } = useProjectTarget();

  if (!projectId) {
    return (
      <div className="empty-state">
        Select a project to begin grading images
      </div>
    );
  }

  return <GroupedImageGrid useLazyImages={true} />;
}