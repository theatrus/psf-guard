import { useState, useRef, useEffect } from 'react';

interface ImageWithFallbackProps {
  src: string;
  alt: string;
  className?: string;
  loading?: 'lazy' | 'eager';
  onLoad?: () => void;
  onError?: () => void;
  fallbackContent?: React.ReactNode;
  retryable?: boolean;
  style?: React.CSSProperties;
}

export default function ImageWithFallback({
  src,
  alt,
  className = '',
  loading = 'lazy',
  onLoad,
  onError,
  fallbackContent,
  retryable = true,
  style,
}: ImageWithFallbackProps) {
  const [imageState, setImageState] = useState<'loading' | 'loaded' | 'error'>('loading');
  const [retryCount, setRetryCount] = useState(0);
  const imgRef = useRef<HTMLImageElement>(null);

  // Reset state when src changes
  useEffect(() => {
    setImageState('loading');
    setRetryCount(0);
  }, [src]);

  const handleLoad = () => {
    setImageState('loaded');
    onLoad?.();
  };

  const handleError = () => {
    setImageState('error');
    onError?.();
  };

  const handleRetry = () => {
    if (retryCount < 3) {
      setRetryCount(prev => prev + 1);
      setImageState('loading');
      // Force reload by adding timestamp
      if (imgRef.current) {
        const url = new URL(src, window.location.origin);
        url.searchParams.set('retry', String(Date.now()));
        imgRef.current.src = url.toString();
      }
    }
  };

  const defaultFallback = (
    <div className="image-error-placeholder">
      <div className="error-icon">
        <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5">
          <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
          <line x1="9" y1="9" x2="15" y2="15" />
          <line x1="15" y1="9" x2="9" y2="15" />
        </svg>
      </div>
      <p className="error-message">Image not found</p>
      {retryable && retryCount < 3 && (
        <button className="retry-button" onClick={handleRetry}>
          Retry
        </button>
      )}
    </div>
  );

  return (
    <>
      {imageState === 'loading' && (
        <div className={`image-loading-placeholder ${className}`} style={style}>
          <div className="loading-spinner"></div>
        </div>
      )}
      
      {imageState === 'error' && (
        <div className={`image-error-container ${className}`} style={style}>
          {fallbackContent || defaultFallback}
        </div>
      )}
      
      <img
        ref={imgRef}
        src={src}
        alt={alt}
        className={className}
        loading={loading}
        onLoad={handleLoad}
        onError={handleError}
        style={{
          ...style,
          display: imageState === 'loaded' ? 'block' : 'none',
        }}
      />
    </>
  );
}