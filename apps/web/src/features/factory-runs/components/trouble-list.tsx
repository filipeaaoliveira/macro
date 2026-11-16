import { InsightSection } from '@app/features/scheduling/components/insights-panel';
import { t } from '@app/lib/i18n';
import { For } from 'solid-js';
import { flowWordLabel } from '../core/flow-words';
import type { EntryInsight } from '../core/insights';
import { RunLinkList } from './run-link-list';

/**
 * The entries where Runs failed or went round again, worst first, each with
 * the newest of those Runs to open.
 */
export function TroubleList(props: {
  entries: EntryInsight[];
  onOpenRun: (runId: string) => void;
}) {
  const most = () =>
    Math.max(
      1,
      ...props.entries.map((entry) => entry.failedRuns + entry.revisitedRuns)
    );

  return (
    <InsightSection
      title={t('factoryInsights.whereRunsStop')}
      description={t('factoryInsights.whereRunsStopDescription')}
    >
      <div class="flex flex-col gap-5 p-5">
        <For
          each={props.entries}
          fallback={
            <p class="py-8 text-center text-sm text-ink-muted">
              {t('factoryInsights.nothingStopped')}
            </p>
          }
        >
          {(entry) => {
            const failed = () => entry.failedRuns;
            return (
              <div class="flex flex-col gap-2">
                <div class="flex items-center justify-between gap-3 text-sm">
                  <span class="truncate">{flowWordLabel(entry.entry)}</span>
                  <span class="text-xs text-ink-muted tabular-nums">
                    {t('factoryInsights.failedRuns', { count: failed() })}
                    {' · '}
                    {t('factoryInsights.againRuns', {
                      count: entry.revisitedRuns,
                    })}
                  </span>
                </div>
                <div class="flex h-1.5 overflow-hidden rounded-full bg-surface-2">
                  <div
                    class="h-full bg-failure"
                    style={{ width: `${(failed() / most()) * 100}%` }}
                  />
                  <div
                    class="h-full bg-ink/70"
                    style={{
                      width: `${(entry.revisitedRuns / most()) * 100}%`,
                    }}
                  />
                </div>
                <RunLinkList
                  label={t('factoryInsights.runsAt', {
                    step: flowWordLabel(entry.entry),
                  })}
                  runs={entry.trouble.map((trouble) => ({
                    run: trouble.run,
                    detail: trouble.failed
                      ? t('factoryInsights.failed')
                      : t('factoryInsights.visits', { count: trouble.visits }),
                  }))}
                  onOpenRun={props.onOpenRun}
                />
              </div>
            );
          }}
        </For>
      </div>
    </InsightSection>
  );
}
