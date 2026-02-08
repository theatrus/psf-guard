import { describe, it, expect } from 'vitest';

describe('Test infrastructure', () => {
  it('vitest runs correctly', () => {
    expect(1 + 1).toBe(2);
  });

  it('jsdom environment is active', () => {
    expect(typeof document).toBe('object');
    expect(typeof window).toBe('object');
  });

  it('MSW server is running (setup.ts loaded)', async () => {
    // If MSW setup failed, this would throw due to onUnhandledRequest: 'error'
    const response = await fetch('/api/projects');
    expect(response.ok).toBe(true);
    const json = await response.json();
    expect(json.success).toBe(true);
  });
});
