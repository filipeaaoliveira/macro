/**
 * A Flow's own word (an entry such as `ci-fix`, an outcome such as
 * `approve`) as a label: `Ci fix`, `Approve`. Flows name these, so they are
 * shown as written rather than translated.
 */
export function flowWordLabel(word: string): string {
  const spaced = flowWordText(word);
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

/** A Flow's word inside a sentence: `ci fix`, as in "Running ci fix". */
export function flowWordText(word: string): string {
  return word.replace(/[-_]+/g, ' ').trim();
}
