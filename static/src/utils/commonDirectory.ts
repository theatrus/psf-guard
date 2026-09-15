/**
 * The longest folder every directory shares, as the export's reference
 * placement names frames below it. Empty when they share none.
 */
export function commonDirectory(directories: string[]): string {
  if (directories.length === 0) return '';
  const separator = directories[0].includes('\\') && !directories[0].includes('/') ? '\\' : '/';
  const split = directories.map((dir) => dir.split(/[\\/]+/).filter((part, i) => part || i === 0));
  const common: string[] = [];
  for (let i = 0; ; i++) {
    const part = split[0][i];
    if (part === undefined || split.some((parts) => parts[i] !== part)) break;
    common.push(part);
  }
  if (common.length === 0 || (common.length === 1 && common[0] === '')) return '';
  return common.join(separator) || separator;
}
