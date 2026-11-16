import { InsightSection } from '@app/features/scheduling/components/insights-panel';
import { t } from '@app/lib/i18n';
import { For, type JSX, Show } from 'solid-js';
import { flowWordLabel } from '../core/flow-words';
import { type EntryInsight, tokenTotal } from '../core/insights';
import { formatInsightDuration, formatTokens } from './insight-format';

function Figure(props: { label: string; children: JSX.Element }) {
  return (
    <div class="flex min-w-0 flex-col gap-0.5">
      <dt class="text-xs text-ink-muted">{props.label}</dt>
      <dd class="text-sm font-medium tabular-nums">{props.children}</dd>
    </div>
  );
}

/** `done 3 · failed 1`: a Flow's words, as it writes them, with counts. */
function Counts(props: {
  label: string;
  counts: { name: string; count: number }[];
}) {
  return (
    <Show when={props.counts.length > 0}>
      <p class="text-xs text-ink-muted">
        <span class="sr-only">{props.label}: </span>
        <For each={props.counts}>
          {(count, index) => (
            <>
              <Show when={index() > 0}>{' · '}</Show>
              <span>{count.name}</span>{' '}
              <span class="tabular-nums text-ink">{count.count}</span>
            </>
          )}
        </For>
      </p>
    </Show>
  );
}

/**
 * Each entry the Runs visited, in its Flow's order: how often, how long its
 * Steps took, how they ended, how long its Gate waited, and what its agent
 * used. A review that sends work to fix, or CI that needs a fix, shows in
 * its outcomes and in the Runs that went round again.
 */
export function EntryList(props: { entries: EntryInsight[] }) {
  return (
    <InsightSection
      title={t('factoryInsights.steps')}
      description={t('factoryInsights.stepsDescription')}
    >
      <ul class="flex flex-col divide-y divide-edge-muted">
        <For
          each={props.entries}
          fallback={
            <li class="py-8 text-center text-sm text-ink-muted">
              {t('factoryInsights.noSteps')}
            </li>
          }
        >
          {(entry) => (
            <li class="flex flex-col gap-3 px-4 py-4 sm:px-5">
              <div class="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
                <h3 class="text-sm font-medium">
                  {flowWordLabel(entry.entry)}
                </h3>
                <span class="text-xs text-ink-muted">
                  {t('factoryInsights.stepsCount', { count: entry.steps })}
                  {' · '}
                  {t('factoryInsights.runsCount', { count: entry.runs })}
                </span>
              </div>
              <dl class="grid grid-cols-2 gap-x-4 gap-y-3 @md/insight-main:grid-cols-5">
                <Figure label={t('factoryInsights.medianTime')}>
                  {entry.duration
                    ? formatInsightDuration(entry.duration.medianSeconds)
                    : '–'}
                </Figure>
                <Figure label={t('factoryInsights.slowestTime')}>
                  {entry.duration
                    ? formatInsightDuration(entry.duration.p90Seconds)
                    : '–'}
                </Figure>
                <Figure label={t('factoryInsights.wentAgain')}>
                  {t('factoryInsights.runsCount', {
                    count: entry.revisitedRuns,
                  })}
                </Figure>
                <Figure label={t('factoryInsights.gateWait')}>
                  {entry.gate?.wait
                    ? formatInsightDuration(entry.gate.wait.medianSeconds)
                    : '–'}
                </Figure>
                <Figure label={t('factoryInsights.tokens')}>
                  {entry.tokens ? formatTokens(tokenTotal(entry.tokens)) : '–'}
                </Figure>
              </dl>
              <Counts
                label={t('factoryInsights.stepOutcomes')}
                counts={entry.outcomes}
              />
              <Show when={entry.gate}>
                {(gate) => (
                  <Counts
                    label={t('factoryInsights.gateAnswers')}
                    counts={gate().answers}
                  />
                )}
              </Show>
            </li>
          )}
        </For>
      </ul>
    </InsightSection>
  );
}
