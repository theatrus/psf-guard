import { useId, useState, type ReactNode } from 'react';
import { ListFilter, MoreHorizontal, SlidersHorizontal, X } from 'lucide-react';
import ToolbarMenuButton from './ToolbarMenuButton';
import './toolbar.css';

interface ViewToolbarProps {
  /** What the toolbar is for, as a screen reader names it. */
  label: string;
  className?: string;
  /** Stays at the top of the scrolling view. */
  sticky?: boolean;
  /** The scope the view is narrowed to, as a ScopeChip: first in the
   *  control row and in sight at every width. Clear removes it too. */
  scope?: ReactNode;
  /** Filter pills that narrow what the view shows. */
  filters?: ReactNode;
  /** How many filters narrow the view: the Filters button counts them,
   *  and Clear appears once there is one. */
  activeFilters?: number;
  onClearFilters?: () => void;
  /** Sliders and their buttons, such as the thumbnail size and the score
   *  cutoff: in the control row on a wide screen, folded with the filters
   *  on a phone. */
  controls?: ReactNode;
  /** Controls in the control row at every width, such as Sequence's
   *  selection rule. */
  primary?: ReactNode;
  /** What the View panel holds: how the view draws its images. */
  view?: ReactNode;
  /** A View setting off its default, such as "newest first": the View
   *  button is marked set and names it. */
  viewNote?: string;
  /** Buttons that act on the view, at the end of the control row. */
  tools?: ReactNode;
  /** Tools used too rarely to keep in sight, under ⋯. */
  menu?: ReactNode;
  /** The status row's left side: counts, or the current settings. */
  summary?: ReactNode;
  /** The status row's right side: what is selected and what to do with it. */
  selection?: ReactNode;
}

/**
 * The toolbar of a view that lists images: Images, Sequence and the
 * calibration library.
 *
 * One control row holds the filter pills, the sliders and the tools, with
 * View and ⋯ opening panels; a slim status row under it holds the summary
 * and the selection. On a phone the filters and sliders fold behind one
 * Filters button that counts the filters in use. toolbar.css draws both
 * layouts from the same markup.
 */
export default function ViewToolbar({
  label,
  className,
  sticky = false,
  scope,
  filters,
  activeFilters = 0,
  onClearFilters,
  controls,
  primary,
  view,
  viewNote,
  tools,
  menu,
  summary,
  selection,
}: ViewToolbarProps) {
  const panelId = useId();
  const [open, setOpen] = useState(false);
  const foldable = Boolean(filters || controls);
  const classes = [
    'view-toolbar',
    sticky ? 'sticky' : '',
    open ? 'is-open' : '',
    className ?? '',
  ].filter(Boolean).join(' ');

  return (
    <section className={classes} aria-label={label}>
      <div className="view-toolbar-bar">
        {scope && <div className="view-toolbar-scope">{scope}</div>}
        {foldable && (
          <button
            type="button"
            className="tool-button view-toolbar-toggle"
            aria-expanded={open}
            aria-controls={panelId}
            aria-label={activeFilters > 0 ? `Filters (${activeFilters} in use)` : 'Filters'}
            title="Filters and sliders"
            onClick={() => setOpen((value) => !value)}
          >
            <ListFilter size={14} aria-hidden="true" />
            <span className="view-toolbar-toggle-label">Filters</span>
            {activeFilters > 0 && (
              <span className="view-toolbar-count" aria-hidden="true">{activeFilters}</span>
            )}
          </button>
        )}
        {filters && (
          <div id={panelId} className="view-toolbar-filters">
            {filters}
            {onClearFilters && (activeFilters > 0 || Boolean(scope)) && (
              <button
                type="button"
                className="toolbar-pill toolbar-pill-clear"
                aria-label="Clear filters"
                title="Clear filters"
                onClick={onClearFilters}
              >
                <X size={13} aria-hidden="true" />
                Clear
              </button>
            )}
          </div>
        )}
        {controls && (
          <div id={filters ? undefined : panelId} className="view-toolbar-controls">{controls}</div>
        )}
        {primary && <div className="view-toolbar-primary">{primary}</div>}
        {(view || tools || menu) && (
          <div className="view-toolbar-tools">
            {view && (
              <ToolbarMenuButton
                label="View"
                icon={<SlidersHorizontal size={14} aria-hidden="true" />}
                note={viewNote}
                panelClassName="view-toolbar-view"
              >
                {view}
              </ToolbarMenuButton>
            )}
            {tools}
            {menu && (
              <ToolbarMenuButton
                label="More tools"
                icon={<MoreHorizontal size={16} aria-hidden="true" />}
                iconOnly
                closeOnAction
                panelClassName="toolbar-overflow"
              >
                {menu}
              </ToolbarMenuButton>
            )}
          </div>
        )}
      </div>
      {(summary || selection) && (
        <div className="view-toolbar-status">
          <div className="view-toolbar-summary">{summary}</div>
          {selection}
        </div>
      )}
    </section>
  );
}
