import type { StackColorChannelCoverage, StackColorCrop, StackColorJob } from '../api/types';

export const cropOrder: StackColorCrop[] = ['none', 'bounds', 'inscribed'];

export const cropLabels: Record<StackColorCrop, string> = {
  none: 'Keep blank edges',
  bounds: 'Trim to covered box',
  inscribed: 'Trim to full coverage',
};

/// What the crop kept, or null when the preview kept the whole grid.
export function describeCrop(job: StackColorJob): string | null {
  const report = job.crop_report;
  if (!report) return null;
  const percent = Math.round(report.retained_fraction * 100);
  return `Cropped to ${report.width}×${report.height} of ` +
    `${report.grid_width}×${report.grid_height} · ${percent}% kept`;
}

/// Channels sitting far enough from the others to have bounded the crop.
export function offCenterChannels(job: StackColorJob): StackColorChannelCoverage[] {
  return (job.crop_report?.channels ?? []).filter((channel) => channel.off_center);
}
