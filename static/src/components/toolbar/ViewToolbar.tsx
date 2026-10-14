import { useId, useState, type ReactNode } from 'react';
import { SlidersHorizontal } from 'lucide-react';
import './toolbar.css';

interface ViewToolbarProps {
  /** What the toolbar is for, as a screen reader names it. */
  label: string;
  className?: string;
  /** Stays at the top of the scrolling view. */
  sticky?: boolean;
  /** Fields that narrow what the view shows, on a row of their own. */
  filters?: ReactNode;
  /** How many filters narrow the view: the Filters button counts them,
   *  and Clear filters waits for one. */
  activeFilters?: number;
  onClearFilters?: () => void;
  /** Settings on the tools' row: how the view draws its images, and a
   *  view's lone filter, which needs no row of its own. Folds with the
   *  filters. */
  view?: ReactNode;
  /** Buttons that act on the view. They stay in sight at every width. */
  tools?: ReactNode;
  /** The last row's left side: counts, or the controls that choose a
   *  selection. */
  summary?: ReactNode;
  /** The last row's right side: what is selected and what to do with it. */
  selection?: ReactNode;
}

/**
 * The toolbar of a view that lists images: Images, Sequence and the
 * calibration library.
 *
 * On a wide screen the filters take the first row, the view settings and
 * the tools the second, and the summary and the selection the last. On a
 * narrow one the filters and view settings fold behind one Filters button
 * that counts the filters in use; the tools, the summary and the selection
 * stay in sight. toolbar.css draws both layouts from the same markup.
 */
export default function ViewToolbar({
  label,
  className,
  sticky = false,
  filters,
  activeFilters = 0,
  onClearFilters,
  view,
  tools,
  summary,
  selection,
}: ViewToolbarProps) {
  const panelId = useId();
  const [open, setOpen] = useState(false);
  const foldable = Boolean(filters || view);
  const classes = [
    'view-toolbar',
    sticky ? 'sticky' : '',
    view ? 'has-view' : '',
    open ? 'is-open' : '',
    className ?? '',
  ].filter(Boolean).join(' ');

  return (
    <section className={classes} aria-label={label}>
      {foldable && (
        <button
          type="button"
          className="tool-button view-toolbar-toggle"
          aria-expanded={open}
          aria-controls={panelId}
          aria-label={activeFilters > 0 ? `Filters (${activeFilters} in use)` : 'Filters'}
          onClick={() => setOpen((value) => !value)}
        >
          <SlidersHorizontal size={14} aria-hidden="true" />
          Filters
          {activeFilters > 0 && (
            <span className="view-toolbar-count" aria-hidden="true">{activeFilters}</span>
          )}
        </button>
      )}
      {foldable && (
        <div id={panelId} className="view-toolbar-panel">
          {filters && (
            <div className="view-toolbar-filters">
              {filters}
              {onClearFilters && (
                <button
                  type="button"
                  className="tool-button tool-button-quiet view-toolbar-clear"
                  disabled={activeFilters === 0}
                  onClick={onClearFilters}
                >
                  Clear filters
                </button>
              )}
            </div>
          )}
          {view && <div className="view-toolbar-view">{view}</div>}
        </div>
      )}
      {tools && <div className="view-toolbar-tools">{tools}</div>}
      {(summary || selection) && (
        <div className="view-toolbar-status">
          {summary && <div className="view-toolbar-summary">{summary}</div>}
          {selection}
        </div>
      )}
    </section>
  );
}
