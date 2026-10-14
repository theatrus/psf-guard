import { useId, useRef, type KeyboardEvent } from 'react';
import { ArrowDownNarrowWide, ArrowDownWideNarrow } from 'lucide-react';
import {
  IMAGE_ORDER_LABELS,
  type ImageOrder,
} from '../../utils/imageOrder';
import './toolbar.css';

const ORDERS: { order: ImageOrder; icon: typeof ArrowDownNarrowWide }[] = [
  { order: 'oldest', icon: ArrowDownNarrowWide },
  { order: 'newest', icon: ArrowDownWideNarrow },
];

const STEPS: Record<string, number> = { ArrowLeft: -1, ArrowUp: -1, ArrowRight: 1, ArrowDown: 1 };

interface ImageOrderToggleProps {
  order: ImageOrder;
  onChange: (order: ImageOrder) => void;
  /** What the choice turns around in this view, as its tooltip. */
  title: string;
}

/**
 * Oldest first or newest first, for a View panel: a caption over two
 * choices side by side, like the Library's Compact / Detailed. The choices
 * are radios, so the arrow keys move between them and the view's own
 * arrow and Space shortcuts leave them alone.
 */
export default function ImageOrderToggle({ order, onChange, title }: ImageOrderToggleProps) {
  const captionId = useId();
  const buttons = useRef<(HTMLButtonElement | null)[]>([]);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const step = STEPS[event.key];
    if (!step) return;
    event.preventDefault();
    const index = ORDERS.findIndex((choice) => choice.order === order);
    const next = (index + step + ORDERS.length) % ORDERS.length;
    onChange(ORDERS[next].order);
    buttons.current[next]?.focus();
  };

  return (
    <div className="toolbar-field">
      <span className="toolbar-caption" id={captionId}>Sort</span>
      <div
        className="tool-segmented image-order-toggle"
        role="radiogroup"
        aria-labelledby={captionId}
        title={title}
        onKeyDown={onKeyDown}
      >
        {ORDERS.map(({ order: choice, icon: Icon }, index) => (
          <button
            key={choice}
            ref={(node) => { buttons.current[index] = node; }}
            type="button"
            role="radio"
            aria-checked={order === choice}
            tabIndex={order === choice ? 0 : -1}
            onClick={() => onChange(choice)}
          >
            <Icon size={14} aria-hidden="true" />
            {IMAGE_ORDER_LABELS[choice]}
          </button>
        ))}
      </div>
    </div>
  );
}
