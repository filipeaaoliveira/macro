/** Words Flows use that people write in capitals. */
const ACRONYMS = new Set(['ai', 'ci', 'pr']);

/**
 * A Flow's own word (an entry such as `ci-fix`, an outcome such as
 * `approve`) as a label: `CI fix`, `Approve`. Flows name these, so they are
 * shown as written rather than translated, acronyms in capitals.
 */
export function flowWordLabel(word: string): string {
  const spaced = flowWordText(word);
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

/** A Flow's word inside a sentence: `CI fix`, as in "Running CI fix". */
export function flowWordText(word: string): string {
  return word
    .replace(/[-_]+/g, ' ')
    .trim()
    .split(' ')
    .map((part) => (ACRONYMS.has(part) ? part.toUpperCase() : part))
    .join(' ');
}
