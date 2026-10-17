import * as fs from 'fs';
import * as path from 'path';
import { describe, expect, it } from 'vitest';

/**
 * Automatic work waiting behind a person's is normal and constant, so the
 * job line draws it muted; the warning colour is kept for work that failed
 * or needs a person.
 */

const SRC = path.resolve(__dirname, '../../..');
const theme = fs.readFileSync(path.join(SRC, 'App.css'), 'utf8');
const header = fs.readFileSync(path.join(SRC, 'components/header/header.css'), 'utf8');

/** Every value a theme gives the token, in each `:root` block that sets it. */
function values(token: string): string[] {
  return Array.from(theme.matchAll(new RegExp(`${token}:\\s*([^;]+);`, 'g')), (match) => match[1].trim());
}

describe('job line colours', () => {
  it('draws waiting work without the warning colour', () => {
    for (const token of ['--state-waiting-fg', '--state-waiting-border', '--state-waiting-fill']) {
      const set = values(token);
      expect(set.length, token).toBeGreaterThan(0);
      for (const value of set) expect(value, token).not.toMatch(/warning|error/);
    }
  });

  it('keeps the warning colour for work that needs a person', () => {
    expect(values('--state-attention-border')).toEqual(['var(--color-warning)']);
    expect(header).toMatch(/\.activity-row\[data-attention='true'\]\s*\{[^}]*--state-attention-border/);
    // Waiting rows read the waiting tokens only.
    const waiting = Array.from(header.matchAll(/\[data-job-state='waiting'\][^{]*\{([^}]*)\}/g), (m) => m[1]).join('\n');
    expect(waiting).not.toMatch(/warning|attention/);
  });
});
