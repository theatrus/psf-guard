import { describe, expect, it } from 'vitest';
import { nextTuck, TOP_PX, TRAVEL_PX } from '../useTuckAway';

const HEADER = 96;
const ROOM = 5000;

describe('nextTuck', () => {
  it('tucks after the page travels down, and returns after it travels up as far', () => {
    let state = { tucked: false, anchor: 200 };
    state = nextTuck(state, 200 + TRAVEL_PX - 1, ROOM, HEADER);
    expect(state.tucked).toBe(false);
    state = nextTuck(state, 200 + TRAVEL_PX, ROOM, HEADER);
    expect(state.tucked).toBe(true);
    // Further down moves the turning point with it.
    state = nextTuck(state, 600, ROOM, HEADER);
    expect(state).toEqual({ tucked: true, anchor: 600 });
    state = nextTuck(state, 600 - TRAVEL_PX + 1, ROOM, HEADER);
    expect(state.tucked).toBe(true);
    state = nextTuck(state, 600 - TRAVEL_PX, ROOM, HEADER);
    expect(state.tucked).toBe(false);
  });

  it('always shows the header near the top', () => {
    expect(nextTuck({ tucked: true, anchor: 300 }, TOP_PX, ROOM, HEADER).tucked).toBe(false);
    expect(nextTuck({ tucked: false, anchor: 0 }, TOP_PX, ROOM, HEADER).tucked).toBe(false);
  });

  it('keeps the header on a page with too little room below to tuck it', () => {
    expect(nextTuck({ tucked: false, anchor: 100 }, 200, HEADER, HEADER).tucked).toBe(false);
    expect(nextTuck({ tucked: false, anchor: 100 }, 200, HEADER + 1, HEADER).tucked).toBe(true);
  });

  it('a turn upward before tucking moves the starting point', () => {
    const state = nextTuck({ tucked: false, anchor: 300 }, 290, ROOM, HEADER);
    expect(state).toEqual({ tucked: false, anchor: 290 });
    expect(nextTuck(state, 290 + TRAVEL_PX, ROOM, HEADER).tucked).toBe(true);
  });
});
