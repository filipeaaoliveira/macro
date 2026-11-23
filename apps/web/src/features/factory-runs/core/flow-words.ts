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

/**
 * Step outcomes that say its work fell short: a failure, checks that stayed
 * red, or Blockers left unresolved. `fail` and `deploy_failed` are kept for
 * the Steps of Runs that went on past their merge, before Runs ended there.
 * Shown in the failure tone.
 */
const SHORT_OUTCOMES = new Set([
  'failed',
  'red',
  'fail',
  'deploy_failed',
  'unresolved',
]);

/**
 * Whether a Step's `outcome` says its work fell short. A Step whose agent
 * stopped without a result (`no_result`) fell short once its owner
 * answered its Gate `stop` (`answer`), as insights count it; retried, or
 * still waiting on them, it did not.
 */
export function fellShort(outcome: string, answer?: string): boolean {
  if (outcome === 'no_result') return answer === 'stop';
  return SHORT_OUTCOMES.has(outcome);
}

/**
 * Step outcomes that send work back to be done again rather than fail it:
 * Blockers a review found, in a plan (`plan-review`) or a change, and a plan
 * that finds its task too big for one Run (`oversized`), which goes to be
 * split. Shown in the warning tone.
 */
const SENT_BACK_OUTCOMES = new Set(['blockers', 'oversized']);

/** How a Step's outcome reads: fell short, sent back, or neither. */
export type OutcomeTone = 'short' | 'sentBack' | undefined;

/**
 * The tone a Step's `outcome` is shown in, its Gate's answer `answer`
 * deciding a `no_result`, as {@link fellShort} does.
 */
export function outcomeTone(outcome: string, answer?: string): OutcomeTone {
  if (fellShort(outcome, answer)) return 'short';
  return SENT_BACK_OUTCOMES.has(outcome) ? 'sentBack' : undefined;
}
