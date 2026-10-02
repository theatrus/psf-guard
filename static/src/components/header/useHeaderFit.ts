import { useLayoutEffect, useState, type RefObject } from 'react';

/** Below this width the header's phone layout always stacks, in CSS. */
const PHONE_QUERY = '(max-width: 760px)';

/** Room a stacked header must have to spare before it goes back to one
 *  row, so a width on the edge, rounded differently in each layout, does
 *  not flip it back and forth. */
const SLACK_PX = 16;

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

    // The width a strip wants: its items side by side, with its gaps,
    // padding and border. Read from the items rather than scrollWidth, which
    // also counts an open popover positioned inside the strip.
    const wanted = (strip: HTMLElement) => {
      const style = getComputedStyle(strip);
      const items = [...strip.children].filter(
        (item) => getComputedStyle(item).position !== 'absolute'
          && getComputedStyle(item).position !== 'fixed'
      );
      const gap = parseFloat(style.columnGap) || 0;
      return items.reduce((sum, item) => sum + item.getBoundingClientRect().width, 0)
        + Math.max(items.length - 1, 0) * gap
        + parseFloat(style.paddingLeft) + parseFloat(style.paddingRight)
        + parseFloat(style.borderLeftWidth) + parseFloat(style.borderRightWidth);
    };

    const measure = () => {
      const brand = part('brand');
      const nav = part('nav');
      const utilities = part('utilities');
      if (!brand || !nav || !utilities) return;
      const apply = (next: boolean) => {
        // Set at once, so a flip never paints a frame of overlap; React
        // renders the same class when the state lands.
        root.classList.toggle('is-stacked', next);
        setStacked(next);
      };
      if (phone.matches) {
        apply(true);
        return;
      }
      const style = getComputedStyle(root);
      // Both layouts share the header's padding, so the room is the same
      // whichever one is showing.
      const padding = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
      apply(
        needsStacking(
          {
            available: root.clientWidth - padding,
            brand: brand.getBoundingClientRect().width,
            nav: wanted(nav),
            utilities: wanted(utilities),
            gap: parseFloat(style.columnGap) || 0,
          },
          root.classList.contains('is-stacked')
        )
      );
    };

    // A width changes when one of the strips' items changes size, so those
    // are what is watched: not every row of an open picker or queue.
    const resize = new ResizeObserver(measure);
    const mutations = new MutationObserver(() => {
      watch();
      measure();
    });
    const watch = () => {
      resize.disconnect();
      mutations.disconnect();
      resize.observe(root);
      for (const strip of root.querySelectorAll<HTMLElement>('[data-header-part]')) {
        resize.observe(strip);
        mutations.observe(strip, { childList: true });
        for (const item of strip.children) resize.observe(item);
      }
      for (const group of root.querySelectorAll<HTMLElement>('[data-header-part="nav"] .header-group')) {
        mutations.observe(group, { childList: true });
        for (const item of group.children) resize.observe(item);
      }
    };
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
