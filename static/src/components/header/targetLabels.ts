/** Short labels for a rig's targets. Mosaic panels share a long start
 *  ("Heart and Soul Nebula Panel 1", "… Panel 2"), which a closed select
 *  cuts off before the part that tells them apart. When every name shares
 *  whole leading words of at least `minShared` characters, each label keeps
 *  only what follows; otherwise names stay whole. */
export function sharedStart(names: string[], minShared = 8): string {
  if (names.length < 2) return '';
  const words = names.map(name => name.split(/\s+/));
  const shared: string[] = [];
  for (let i = 0; ; i++) {
    const word = words[0][i];
    // Stop before the last word of any name: every label keeps something.
    if (word === undefined || words.some(w => w.length <= i + 1 || w[i] !== word)) break;
    shared.push(word);
  }
  // Keep a word before a bare number or letter: "Panel 1", not "1".
  const bare = (name: string[]) => /^[\w]{1,2}$/.test(name[shared.length] ?? '') && name.length === shared.length + 1;
  if (shared.length > 0 && words.some(bare)) shared.pop();
  const prefix = shared.join(' ');
  return prefix.length >= minShared ? prefix : '';
}

export function shortLabel(name: string, prefix: string): string {
  return prefix && name.startsWith(`${prefix} `) ? name.slice(prefix.length + 1) : name;
}

/** A width for a closed select that shows `label` whole, within reason. */
export function fittedWidth(label: string, minRem = 7, maxRem = 24): string {
  return `clamp(${minRem}rem, calc(${label.length}ch + 2.75rem), ${maxRem}rem)`;
}
