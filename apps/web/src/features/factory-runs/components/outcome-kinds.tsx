import {
  InsightSection,
  Metric,
} from '@app/features/scheduling/components/insights-panel';
import { t } from '@app/lib/i18n';
import {
  flawedRate,
  infrastructureRate,
  type OutcomeCounts,
} from '../core/insights';

/** A share as a whole percentage, none while it has no value. */
function percent(share: number | undefined): number | undefined {
  return share === undefined ? undefined : Math.round(share * 100);
}

/**
 * The factory's quality apart from its plumbing: Runs that failed for their
 * work, Runs that failed for a machine, quota or network reason, and Runs
 * that succeeded but whose result was found wrong afterwards.
 */
export function OutcomeKinds(props: { totals: OutcomeCounts }) {
  return (
    <InsightSection
      title={t('factoryInsights.outcomeKinds')}
      description={t('factoryInsights.outcomeKindsDescription')}
    >
      <div class="grid grid-cols-1 divide-y divide-edge-muted @xs/insight-side:grid-cols-2 @xs/insight-side:divide-x @xs/insight-side:divide-y-0">
        <Metric
          label={t('factoryInsights.flawedRate')}
          definition={t('factoryInsights.flawedRateDefinition')}
          value={percent(flawedRate(props.totals))}
          unit="%"
          lowerIsBetter
        />
        <Metric
          label={t('factoryInsights.infrastructureRate')}
          definition={t('factoryInsights.infrastructureRateDefinition')}
          value={percent(infrastructureRate(props.totals))}
          unit="%"
          lowerIsBetter
        />
      </div>
      <dl class="grid grid-cols-3 gap-3 px-4 pb-4 pt-2 text-xs">
        <div class="flex flex-col gap-0.5">
          <dt class="text-ink-muted">{t('factoryInsights.failedForWork')}</dt>
          <dd class="tabular-nums">{props.totals.failed}</dd>
        </div>
        <div class="flex flex-col gap-0.5">
          <dt class="text-ink-muted">
            {t('factoryInsights.failedForInfrastructure')}
          </dt>
          <dd class="tabular-nums">{props.totals.infrastructure}</dd>
        </div>
        <div class="flex flex-col gap-0.5">
          <dt class="text-ink-muted">{t('factoryInsights.flawedRuns')}</dt>
          <dd class="tabular-nums">{props.totals.flawed}</dd>
        </div>
      </dl>
    </InsightSection>
  );
}
