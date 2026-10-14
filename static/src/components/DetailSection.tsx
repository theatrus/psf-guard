import { useId, type ReactNode, type Ref } from 'react';
import { ChevronRight } from 'lucide-react';
import './DetailSection.css';

interface DetailSectionProps {
  title: ReactNode;
  /** Drawn before the title, such as the calibration flask. */
  icon?: ReactNode;
  /** The panel's status pill, shown open or folded. */
  badge?: ReactNode;
  /** One line on the heading row while the section is folded, so a folded
   *  panel still says what it holds. */
  summary?: ReactNode;
  open: boolean;
  /** Leave out for a heading that does not fold. */
  onToggle?: () => void;
  /** Classes for the section, such as its border and spacing. */
  className?: string;
  /** Classes for the heading row, for panels that style their own. */
  headingClassName?: string;
  'data-testid'?: string;
  ref?: Ref<HTMLElement>;
  /** Rendered only while open, so a folded panel does no work. */
  children?: ReactNode;
}

/**
 * One panel of the image detail's sidebar, whose heading folds it away.
 * The heading is a button inside the `h3`, so it keeps the heading's place
 * in the outline and takes Enter and Space like any button.
 */
export default function DetailSection({
  title,
  icon,
  badge,
  summary,
  open,
  onToggle,
  className,
  headingClassName,
  'data-testid': testId,
  ref,
  children,
}: DetailSectionProps) {
  const bodyId = useId();
  const label = (
    <span className="detail-section-title">
      {icon}
      {title}
    </span>
  );
  return (
    <section
      ref={ref}
      className={[
        'info-section',
        'detail-section',
        open ? null : 'is-folded',
        className,
      ].filter(Boolean).join(' ')}
      data-testid={testId}
    >
      <div className={['detail-section-heading', headingClassName].filter(Boolean).join(' ')}>
        <h3>
          {onToggle ? (
            <button
              type="button"
              className="detail-section-toggle"
              aria-expanded={open}
              aria-controls={bodyId}
              onClick={onToggle}
            >
              <ChevronRight className="detail-section-chevron" size={16} aria-hidden="true" />
              {label}
              {!open && summary != null && summary !== '' && (
                <>
                  {' '}
                  <span
                    className="detail-section-summary"
                    title={typeof summary === 'string' ? summary : undefined}
                  >
                    <span aria-hidden="true">· </span>
                    {summary}
                  </span>
                </>
              )}
            </button>
          ) : (
            label
          )}
        </h3>
        {badge}
      </div>
      <div id={bodyId} className="detail-section-body" hidden={!open}>
        {open && children}
      </div>
    </section>
  );
}
