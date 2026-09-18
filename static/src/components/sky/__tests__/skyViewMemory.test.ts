import { beforeEach, describe, expect, it } from 'vitest';
import { forgetSkyView, recallSkyView, rememberSkyView } from '../skyViewMemory';

describe('skyViewMemory', () => {
  beforeEach(() => {
    forgetSkyView();
    window.sessionStorage.clear();
  });

  it('remembers what it is told, merging later changes', () => {
    expect(recallSkyView()).toEqual({});
    rememberSkyView({ frame: 'galactic', zoom: 4 });
    rememberSkyView({ centerLon: 315.2, centerLat: 67.9 });
    expect(recallSkyView()).toEqual({ frame: 'galactic', zoom: 4, centerLon: 315.2, centerLat: 67.9 });
    expect(JSON.parse(window.sessionStorage.getItem('psf-guard:sky-view:v1') ?? '{}').zoom).toBe(4);
  });

  it('survives a fresh module read and shrugs off a broken record', () => {
    rememberSkyView({ mode: 'globe' });
    forgetSkyView();
    expect(recallSkyView()).toEqual({});
    window.sessionStorage.setItem('psf-guard:sky-view:v1', 'not json');
    forgetSkyView();
    window.sessionStorage.setItem('psf-guard:sky-view:v1', 'not json');
    expect(recallSkyView()).toEqual({});
  });
});
