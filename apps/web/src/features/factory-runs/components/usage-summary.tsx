import {
  InsightSection,
  Metric,
} from '@app/features/scheduling/components/insights-panel';
import { t } from '@app/lib/i18n';
import { Show } from 'solid-js';
import type { RunInsights } from '../core/insights';
import { tokenTotal } from '../core/insights';
import { formatTokens } from './insight-format';
import { RunLinkList } from './run-link-list';

/**
 * The tokens the period's agent Steps used, as Macro recorded them, and the
 * Runs that used the most. An agent on a person's own computer records none,
 * which the note says.
 */
export function UsageSummary(props: {
  usage: RunInsights['usage'];
  onOpenRun: (runId: string) => void;
}) {
  const total = () => tokenTotal(props.usage.tokens);
  const perRun = () =>
    props.usage.measuredRuns > 0 ? total() / props.usage.measuredRuns : 0;

  return (
    <InsightSection
      title={t('factoryInsights.agentUsage')}
      description={t('factoryInsights.agentUsageDescription')}
    >
      <div class="grid grid-cols-1 divide-y divide-edge-muted @xs/insight-side:grid-cols-2 @xs/insight-side:divide-x @xs/insight-side:divide-y-0">
        <Metric
          label={t('factoryInsights.tokens')}
          definition={t('factoryInsights.tokensDefinition')}
          value={total()}
        />
        <Metric
          label={t('factoryInsights.tokensPerRun')}
          definition={t('factoryInsights.tokensPerRunDefinition')}
          value={Math.round(perRun())}
        />
      </div>
      <div class="flex flex-col gap-3 px-4 pb-4 pt-2">
        <p class="text-xs text-ink-muted">
          {t('factoryInsights.measuredSteps', {
            measured: props.usage.measuredSteps,
            count: props.usage.agentSteps,
          })}
        </p>
        <Show when={props.usage.heaviest.length > 0}>
          <RunLinkList
            label={t('factoryInsights.heaviestRuns')}
            runs={props.usage.heaviest.map((heavy) => ({
              run: heavy.run,
              detail: formatTokens(tokenTotal(heavy.tokens)),
            }))}
            onOpenRun={props.onOpenRun}
          />
        </Show>
      </div>
    </InsightSection>
  );
}
