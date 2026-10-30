import { useId, type CSSProperties, type ReactNode, type Ref } from 'react';

interface ImageViewerFrameProps {
  /** The image's name, as the header's heading. */
  title: ReactNode;
  /** Centred in the header, such as a frame's grade. */
  banner?: ReactNode;
  closeLabel?: string;
  onClose: () => void;
  /** The zoomable image, beside the sidebar. */
  image: ReactNode;
  imageRef?: Ref<HTMLDivElement>;
  imageStyle?: CSSProperties;
  /** The sidebar's panels, its shortcuts and its view controls. */
  children: ReactNode;
  sidebarRef?: Ref<HTMLDivElement>;
  /** Under the image and the sidebar, such as a phone's grading bar. */
  footer?: ReactNode;
  /** Beside the viewer, such as a dialog it opens. */
  after?: ReactNode;
  className?: string;
  overlayClassName?: string;
  /** A modal dialog named by its heading, for a viewer opened over a page. */
  dialog?: boolean;
}

/**
 * The frame both image viewers share, the image reviewer's and the stack
 * inspector's: the whole window, with the image's name above, the image on
 * the left and a sidebar on the right. A phone puts the sidebar under the
 * image.
 */
export default function ImageViewerFrame({
  title,
  banner,
  closeLabel,
  onClose,
  image,
  imageRef,
  imageStyle,
  children,
  sidebarRef,
  footer,
  after,
  className,
  overlayClassName,
  dialog = false,
}: ImageViewerFrameProps) {
  const titleId = useId();
  return (
    <div
      className={['image-detail-overlay', overlayClassName].filter(Boolean).join(' ')}
      role={dialog ? 'presentation' : undefined}
      onClick={onClose}
    >
      <div
        className={['image-detail', className].filter(Boolean).join(' ')}
        onClick={(event) => event.stopPropagation()}
        {...(dialog ? { role: 'dialog', 'aria-modal': true, 'aria-labelledby': titleId } : {})}
      >
        <div className="detail-header">
          <h2 id={titleId}>{title}</h2>
          {banner}
          <button className="close-button" type="button" onClick={onClose} aria-label={closeLabel}>×</button>
        </div>
        <div className="detail-content">
          <div className="detail-image" ref={imageRef} style={imageStyle}>
            {image}
          </div>
          <div className="detail-info" ref={sidebarRef}>
            {children}
          </div>
        </div>
        {footer}
      </div>
      {after}
    </div>
  );
}
