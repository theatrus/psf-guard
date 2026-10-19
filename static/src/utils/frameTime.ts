import type { Image } from '../api/types';

/** When a frame was captured, as the suspect lists and the pointing chart
 *  name it. */
export function frameTime(image: Image | undefined): string {
  if (!image?.acquired_date) return 'No time';
  return new Date(image.acquired_date * 1000).toLocaleString([], {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  });
}
