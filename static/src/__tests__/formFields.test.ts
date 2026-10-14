import * as fs from 'fs';
import * as path from 'path';
import { describe, expect, it } from 'vitest';

/**
 * Native form fields follow the theme (see the Styling section of
 * static/README.md). A theme block in App.css says which color-scheme the
 * browser should draw its own parts of a control in, and gives every
 * --color-input-* token a value; index.css draws text, number, search and
 * date inputs, selects and textareas from those tokens at no specificity.
 * Without either, the browser falls back to its own field colours, which
 * are white whenever the operating system is in light mode.
 */

const SRC = path.resolve(__dirname, '..');

function stylesheets(directory: string, out: string[] = []): string[] {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      if (entry.name !== '__tests__') stylesheets(full, out);
    } else if (entry.name.endsWith('.css')) {
      out.push(full);
    }
  }
  return out;
}

function read(file: string): string {
  return fs.readFileSync(file, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
}

/** Every leaf block: its selector and its declarations by property. */
function rules(text: string): { selector: string; declarations: Map<string, string> }[] {
  return [...text.matchAll(/([^{}]*)\{([^{}]*)\}/g)].map((match) => {
    const declarations = new Map<string, string>();
    for (const declaration of match[2].split(';')) {
      const at = declaration.indexOf(':');
      if (at < 0) continue;
      declarations.set(
        declaration.slice(0, at).trim(),
        declaration.slice(at + 1).trim().replace(/\s+/g, ' '),
      );
    }
    return { selector: match[1].trim().replace(/\s+/g, ' '), declarations };
  });
}

const all = stylesheets(SRC).map((file) => ({ name: path.relative(SRC, file), text: read(file) }));
const app = rules(read(path.join(SRC, 'App.css')));
const base = rules(read(path.join(SRC, 'index.css')));

// A theme block is one that defines the palette.
const themes = app.filter(({ declarations }) => declarations.has('--color-bg-primary'));
const inputTokens = [
  ...new Set(all.flatMap(({ text }) => [...text.matchAll(/var\((--color-input-[\w-]+)\)/g)].map((m) => m[1]))),
].sort();

/**
 * The base rule for every field in `state`, written wholly inside :where()
 * so that it has no specificity. index.css loads after App.css, so a state
 * outside :where() would beat a component's own rule of one class.
 */
function fieldRule(state = '') {
  return base.find(({ selector }) => {
    const inner = /^:where\((.*)\)$/.exec(selector)?.[1];
    // Split on the commas that are not inside :not(…).
    const parts = inner?.split(/,(?![^()]*\))/).map((part) => part.trim()) ?? [];
    return (
      parts.length === 3 &&
      parts[0].startsWith("input:not([type='checkbox'], [type='radio'], [type='range']") &&
      parts[0].endsWith(`)${state}`) &&
      parts[1] === `select${state}` &&
      parts[2] === `textarea${state}`
    );
  });
}

describe('form fields', () => {
  it('has a theme that tells the browser its color-scheme', () => {
    expect(themes.length).toBeGreaterThan(0);
    for (const { selector, declarations } of themes) {
      expect(declarations.get('color-scheme'), `${selector} declares no color-scheme`).toMatch(/^(dark|light)$/);
    }
  });

  it('sets color-scheme only beside a theme’s tokens', () => {
    const elsewhere = all.flatMap(({ name, text }) =>
      rules(text)
        .filter(({ declarations }) => declarations.has('color-scheme') && !declarations.has('--color-bg-primary'))
        .map(({ selector, declarations }) => `${name}: ${selector} { color-scheme: ${declarations.get('color-scheme')} }`),
    );
    expect(elsewhere).toEqual([]);
  });

  it('gives every input token a value in every theme', () => {
    expect(inputTokens).toEqual(
      expect.arrayContaining(['--color-input-bg', '--color-input-border', '--color-input-text']),
    );
    for (const { selector, declarations } of themes) {
      const missing = inputTokens.filter((token) => !declarations.has(token));
      expect(missing, `${selector} leaves these input tokens unset`).toEqual([]);
    }
  });

  it('draws native fields from the input tokens at no specificity', () => {
    const declarations = (state = '') => {
      const rule = fieldRule(state);
      expect(rule, `index.css needs :where(input:not(…)${state}, select${state}, textarea${state})`).toBeDefined();
      return Object.fromEntries(rule!.declarations);
    };
    expect(declarations()).toMatchObject({
      'background-color': 'var(--color-input-bg)',
      color: 'var(--color-input-text)',
      border: '1px solid var(--color-input-border)',
    });
    expect(declarations(':focus-visible')).toMatchObject({
      outline: '1px solid var(--state-focus-border)',
      'border-color': 'var(--state-focus-border)',
      'box-shadow': 'var(--state-focus-ring)',
    });
    expect(declarations("[aria-invalid='true']")).toMatchObject({
      'border-color': 'var(--color-input-invalid-border)',
      'outline-color': 'var(--color-input-invalid-border)',
    });
    expect(declarations(':disabled')).toMatchObject({
      'background-color': 'var(--color-input-disabled-bg)',
      color: 'var(--color-input-disabled-text)',
      'border-color': 'var(--color-input-disabled-border)',
    });

    const placeholder = base.find(({ selector }) => selector === ':where(input, textarea)::placeholder');
    expect(placeholder?.declarations.get('color')).toBe('var(--color-input-placeholder)');
  });

  it('keeps an invalid field’s colour while it has focus', () => {
    const order = [':focus-visible', "[aria-invalid='true']"].map((state) => base.indexOf(fieldRule(state)!));
    expect(order[0]).toBeGreaterThanOrEqual(0);
    expect(order[1]).toBeGreaterThan(order[0]);
  });
});
