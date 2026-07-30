export type ImageDetailReturnView = 'grid' | 'sequence';

const RETURN_TO_PARAM = 'returnTo';

export interface SequenceReturnPosition {
  scrollTop: number;
  imageId: number;
  offsetTop: number;
}

export interface ImageDetailNavigationState {
  sequenceReturn: SequenceReturnPosition;
}

export function imageDetailNavigationState(
  sequenceReturn: SequenceReturnPosition,
): ImageDetailNavigationState {
  return {
    sequenceReturn: {
      scrollTop: Math.max(0, sequenceReturn.scrollTop),
      imageId: sequenceReturn.imageId,
      offsetTop: sequenceReturn.offsetTop,
    },
  };
}

export function sequenceReturnPositionFromState(
  state: unknown,
): SequenceReturnPosition | null {
  if (!state || typeof state !== 'object' || !('sequenceReturn' in state)) {
    return null;
  }
  const value = state.sequenceReturn;
  if (!value || typeof value !== 'object') return null;
  if (!('scrollTop' in value) || !('imageId' in value) || !('offsetTop' in value)) {
    return null;
  }
  return typeof value.scrollTop === 'number'
    && Number.isFinite(value.scrollTop)
    && value.scrollTop >= 0
    && typeof value.imageId === 'number'
    && Number.isInteger(value.imageId)
    && typeof value.offsetTop === 'number'
    && Number.isFinite(value.offsetTop)
    ? {
        scrollTop: value.scrollTop,
        imageId: value.imageId,
        offsetTop: value.offsetTop,
      }
    : null;
}

export function imageDetailReturnView(
  searchParams: URLSearchParams,
): ImageDetailReturnView {
  return searchParams.get(RETURN_TO_PARAM) === 'sequence' ? 'sequence' : 'grid';
}

export function imageDetailPath(
  imageId: number,
  searchParams: URLSearchParams,
  returnView: ImageDetailReturnView,
): string {
  const params = new URLSearchParams(searchParams);
  if (returnView === 'sequence') {
    params.set(RETURN_TO_PARAM, 'sequence');
    params.set('current', String(imageId));
  } else {
    params.delete(RETURN_TO_PARAM);
  }
  const query = params.toString();
  return `/detail/${imageId}${query ? `?${query}` : ''}`;
}

export function imageDetailClosePath(searchParams: URLSearchParams): string {
  const params = new URLSearchParams(searchParams);
  const returnView = imageDetailReturnView(params);
  params.delete(RETURN_TO_PARAM);
  const query = params.toString();
  return `/${returnView}${query ? `?${query}` : ''}`;
}
