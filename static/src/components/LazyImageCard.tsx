import ImageCard, { type ImageCardProps } from './ImageCard';

type LazyImageCardProps = Omit<ImageCardProps, 'lazyPreview'>;

export default function LazyImageCard(props: LazyImageCardProps) {
  return <ImageCard {...props} lazyPreview />;
}
