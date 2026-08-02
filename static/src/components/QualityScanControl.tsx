import type { useSpatialScan } from '../hooks/useSpatialScan';

type QualityScan = ReturnType<typeof useSpatialScan>;

interface QualityScanButtonProps {
  scan: QualityScan;
  targetId: number | null | undefined;
  canWrite: boolean;
  className?: string;
}

export function QualityScanButton({
  scan,
  targetId,
  canWrite,
  className,
}: QualityScanButtonProps) {
  const scope = scan.scope;
  if (targetId == null || !scope?.needs_analysis) {
    return null;
  }

  const frameLabel = (count: number) => `${count} ${count === 1 ? 'frame' : 'frames'}`;
  let title = scope.new_frames > 0 && scope.outdated_frames > 0
    ? `Analyze ${frameLabel(scope.new_frames)} and update ${frameLabel(scope.outdated_frames)} for the current quality model.`
    : scope.new_frames > 0
      ? `Analyze ${frameLabel(scope.new_frames)} added since the last quality scan.`
      : `Update ${frameLabel(scope.outdated_frames)} for the current quality model.`;
  if (scan.startError) {
    title = `Quality analysis did not start: ${scan.startError.message}`;
  } else if (scan.isRunning) {
    title = 'A quality scan is already running for this database.';
  } else if (!canWrite) {
    title = 'A read-only account cannot start quality analysis.';
  }

  let label = 'Analyze Quality';
  if (scan.startError) {
    label = 'Analysis failed';
  } else if (scan.isStarting) {
    label = 'Starting analysis…';
  } else if (scan.isRunning) {
    label = 'Analysis running…';
  }

  return (
    <button
      type="button"
      className={className}
      onClick={() => scan.start(undefined)}
      disabled={!canWrite || scan.isStarting || scan.isRunning}
      title={title}
    >
      {label}
    </button>
  );
}
