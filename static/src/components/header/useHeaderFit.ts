import { useLayoutEffect, useState, type RefObject } from 'react';

/** Below this width the header's phone layout always stacks, in CSS. */
const PHONE_QUERY = '(max-width: 760px)';

/** Room a stacked header must have to spare before it goes back to one
 *  row, so a width on the edge does not flip it back and forth. */
const SLACK_PX = 8;

export interface HeaderWidths {
  available: number;
  brand: number;
  nav: number;
  utilities: number;
  gap: number;
}

/** Whether the views need a row of their own. */
export function needsStacking(widths: HeaderWidths, stacked: boolean): boolean {
  const needed = widths.brand + widths.nav + widths.utilities + 2 * widths.gap;
  return needed > widths.available - (stacked ? SLACK_PX : 0);
}

/**
 * Whether the header's views drop to a second row. They share the first row
 * with the brand and the utilities whenever all three fit, whatever the
 * window width: a fixed breakpoint either wrapped a short nav for nothing or
 * squeezed a long target name. Measured from the elements, so a longer
 * target or another chip moves the line where it should.
 */
export function useHeaderFit(header: RefObject<HTMLElement | null>): boolean {
  const [stacked, setStacked] = useState(false);

  useLayoutEffect(() => {
    const root = header.current;
    // Without these (tests, very old browsers) the header stays on one row
    // and the phone CSS still stacks it.
    if (!root || typeof ResizeObserver === 'undefined' || typeof window.matchMedia !== 'function') {
      return;
    }
    const part = (name: string) => root.querySelector<HTMLElement>(`[data-header-part="${name}"]`);
    const phone = window.matchMedia(PHONE_QUERY);

    const measure = () => {
      const brand = part('brand');
      const nav = part('nav');
      const utilities = part('utilities');
      if (!brand || !nav || !utilities) return;
      if (phone.matches) {
        setStacked(true);
        return;
      }
      const style = getComputedStyle(root);
      const padding = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
      setStacked((current) =>
        needsStacking(
          {
            available: root.clientWidth - padding,
            brand: brand.getBoundingClientRect().width,
            // The nav keeps its buttons on one line, so its scroll width is
            // the width it wants even while its column squeezes it.
            nav: nav.scrollWidth,
            utilities: utilities.scrollWidth,
            gap: parseFloat(style.columnGap) || 0,
          },
          current
        )
      );
    };

    // Anything inside can change width: a target name, a chip that comes
    // and goes. Watch every element, and pick up new ones as they mount.
    const resize = new ResizeObserver(measure);
    const watch = () => {
      resize.disconnect();
      resize.observe(root);
      root.querySelectorAll<HTMLElement>('[data-header-part], [data-header-part] *').forEach((element) => {
        resize.observe(element);
      });
    };
    const mutations = new MutationObserver(() => {
      watch();
      measure();
    });
    mutations.observe(root, { childList: true, subtree: true });
    phone.addEventListener('change', measure);
    watch();
    measure();
    return () => {
      resize.disconnect();
      mutations.disconnect();
      phone.removeEventListener('change', measure);
    };
  }, [header]);

  return stacked;
}
