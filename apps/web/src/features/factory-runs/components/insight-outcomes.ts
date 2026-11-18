import { t } from '@app/lib/i18n';
import type { InsightOutcome } from '../core/insights';
import { runOutcomeLabel } from './run-status';

/** The color each Run state takes in every insights chart. */
export const INSIGHT_OUTCOME_COLORS: Record<InsightOutcome, string> = {
  success: 'var(--color-success)',
  manual: 'var(--color-blue)',
  blocked: 'var(--color-warning)',
  failed: 'var(--color-failure)',
  cancelled: 'var(--color-ink-muted)',
  infrastructure: 'var(--color-teal)',
  flawed: 'var(--color-pink)',
  running: 'var(--color-purple)',
};

/** The word for a Run's state, as the Runs list says it. */
export function insightOutcomeLabel(outcome: InsightOutcome): string {
  return outcome === 'running'
    ? t('factoryRuns.running')
    : runOutcomeLabel(outcome);
}
