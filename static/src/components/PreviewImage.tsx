import { useEffect } from 'react';
import { useAsyncImage } from '../hooks/useAsyncImage';
import type { PreviewDescriptor } from '../api/types';

interface PreviewImageProps {
  dbId: string;
  /** Preview/annotated URL from apiClient.getPreviewUrl / getAnnotatedUrl. */
  src: string;
  /** Identifies the artifact for the batch generation-status poll. */
  descriptor: PreviewDescriptor;
  alt: string;
  loading?: 'lazy' | 'eager';
  /** Applied to the rendered <img>. */
  className?: string;
  imgStyle?: React.CSSProperties;
  /** Shown when generation ultimately fails. */
  fallback?: React.ReactNode;
  onReady?: () => void;
}

/**
 * Preview `<img>` that loads optimistically and, on a cache miss (the server's
 * 202), shows a "Generating…" indicator while the shared batch poller waits for
 * the async queue to produce it — then swaps in the image. A cache hit renders
 * immediately with no extra request.
 */
export default function PreviewImage({
  dbId,
  src,
  descriptor,
  alt,
  loading = 'lazy',
  className,
  imgStyle,
  fallback,
  onReady,
}: PreviewImageProps) {
  const img = useAsyncImage(dbId, src, descriptor);

  useEffect(() => {
    if (img.state === 'ready') onReady?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [img.state]);

  const pending = img.state === 'loading' || img.state === 'generating';

  return (
    <>
      {pending && (
        <div className="preview-status-box">
          <div className="loading-spinner" />
          {img.state === 'generating' && (
            <span className="preview-status-label">Generating…</span>
          )}
        </div>
      )}
      {img.state === 'error' && (fallback ?? <PreviewError />)}
      <img
        src={img.src}
        alt={alt}
        loading={loading}
        onLoad={img.onLoad}
        onError={img.onError}
        className={className}
        style={{ ...imgStyle, display: img.state === 'ready' ? undefined : 'none' }}
      />
    </>
  );
}

function PreviewError() {
  return (
    <div className="preview-status-box preview-status-error">
      <svg
        width="32"
        height="32"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
      >
        <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
        <line x1="9" y1="9" x2="15" y2="15" />
        <line x1="15" y1="9" x2="9" y2="15" />
      </svg>
      <span className="preview-status-label">Image not found</span>
    </div>
  );
}
