export type ImageDetailReturnView = 'grid' | 'sequence';

const RETURN_TO_PARAM = 'returnTo';

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
