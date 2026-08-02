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
  let title = 'Analyze FITS pixels, then refresh sequence scores. Shift-click to recompute all cached evidence from the current catalogs.';
  if (scan.startError) {
    title = `Quality analysis did not start: ${scan.startError.message}`;
  } else if (scan.isRunning) {
    title = 'A quality scan is already running for this database.';
  } else if (!canWrite) {
    title = 'A read-only account cannot start quality analysis.';
  } else if (targetId == null) {
    title = 'Choose a target from the header before starting quality analysis.';
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
      onClick={(event) => scan.start(event.shiftKey || undefined)}
      disabled={!canWrite || targetId == null || scan.isStarting || scan.isRunning}
      title={title}
    >
      {label}
    </button>
  );
}
