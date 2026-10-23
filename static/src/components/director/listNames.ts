/** "C925", "C925 and RedCat", "C925, RedCat and Askar". */
export const listNames = (names: string[]) => names.length < 2
  ? names.join('')
  : `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`;
