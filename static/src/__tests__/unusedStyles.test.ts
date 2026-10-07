import * as fs from 'fs';
import * as path from 'path';
import { describe, expect, it } from 'vitest';

/**
 * Dead styles pile up quietly and some cost real time: a spinner every grid
 * card carried, waiting for a class nothing set, kept the browser restyling
 * thousands of cards on each scroll frame. These checks fail when a
 * stylesheet names a class or keyframes that nothing uses.
 */

const SRC = path.resolve(__dirname, '..');

function walk(directory: string, out: string[] = []): string[] {
  for (const entry of fs.readdirSync(directory, { withFileTypes: true })) {
    const full = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      if (entry.name !== '__tests__' && entry.name !== '__fixtures__') walk(full, out);
    } else {
      out.push(full);
    }
  }
  return out;
}

const files = walk(SRC);
const stylesheets = files.filter((file) => file.endsWith('.css'));
const source = files
  .filter((file) => /\.(ts|tsx)$/.test(file) && !/\.test\.tsx?$/.test(file))
  .map((file) => fs.readFileSync(file, 'utf8'))
  .join('\n');
const css = new Map(
  stylesheets.map((file) => [
    path.relative(SRC, file),
    fs.readFileSync(file, 'utf8').replace(/\/\*[\s\S]*?\*\//g, ''),
  ]),
);

// Only words inside string literals can name a class.
const named = new Set<string>();
for (const literal of source.match(/'(?:[^'\\\n]|\\.)*'|"(?:[^"\\\n]|\\.)*"|`(?:[^`\\]|\\.)*`/g) ?? []) {
  for (const word of literal.match(/[A-Za-z_][\w-]*/g) ?? []) named.add(word);
}

// `tone-${state}` or 'is-' + state makes every class with that prefix live.
// The prefixes here build React keys, element ids and test ids instead.
const NOT_CLASS_PREFIXES = new Set(['image-', 'stack-', 'frames-', 'clip-', 'sky-clip-']);
const prefixes = new Set<string>();
for (const match of source.matchAll(/([A-Za-z_][\w-]*-)\$\{/g)) prefixes.add(match[1]);
for (const match of source.matchAll(/['"]([A-Za-z_][\w-]*-)['"]\s*\+/g)) prefixes.add(match[1]);
for (const prefix of NOT_CLASS_PREFIXES) prefixes.delete(prefix);

function isNamed(cls: string): boolean {
  if (named.has(cls)) return true;
  for (const prefix of prefixes) if (cls.startsWith(prefix)) return true;
  return false;
}

describe('stylesheets', () => {
  it('name only classes the code uses', () => {
    const unused: string[] = [];
    for (const [file, text] of css) {
      for (const match of text.matchAll(/([^{}]+)\{/g)) {
        const selector = match[1].trim();
        if (selector.startsWith('@')) continue;
        for (const [, cls] of selector.matchAll(/\.([A-Za-z_][\w-]*)/g)) {
          if (!isNamed(cls)) unused.push(`${file}: .${cls}`);
        }
      }
    }
    expect([...new Set(unused)]).toEqual([]);
  });

  it('define each keyframes once, and only ones in use', () => {
    const defined = new Map<string, string[]>();
    let rules = '';
    for (const [file, text] of css) {
      for (const [, name] of text.matchAll(/@keyframes\s+([\w-]+)/g)) {
        defined.set(name, [...(defined.get(name) ?? []), file]);
      }
      rules += text.replace(/@keyframes\s+[\w-]+/g, '');
    }
    const problems: string[] = [];
    for (const [name, where] of defined) {
      if (where.length > 1) problems.push(`${name} defined ${where.length} times`);
      const used = new RegExp(`animation(?:-name)?\\s*:[^;}]*\\b${name}\\b`).test(rules)
        || new RegExp(`\\b${name}\\b`).test(source);
      if (!used) problems.push(`${name} unused`);
    }
    expect(problems).toEqual([]);
  });
});
