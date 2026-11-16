import { t } from '@app/lib/i18n';
import type { InsightOutcome } from '../core/insights';
import { runOutcomeLabel } from './run-status';

const COLORS: Record<InsightOutcome, string> = {
  success: 'var(--color-success)',
  manual: 'var(--color-blue)',
  blocked: 'var(--color-warning)',
  failed: 'var(--color-failure)',
  cancelled: 'var(--color-ink-muted)',
  running: 'var(--color-purple)',
};

/** The color a Run's state takes in every insights chart. */
export function insightOutcomeColor(outcome: InsightOutcome): string {
  return COLORS[outcome];
}

/** The word for a Run's state, as the Runs list says it. */
export function insightOutcomeLabel(outcome: InsightOutcome): string {
  return outcome === 'running'
    ? t('factoryRuns.running')
    : runOutcomeLabel(outcome);
}
