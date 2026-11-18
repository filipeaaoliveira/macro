import { ActivityRow } from '@app/features/agent-session-activity/components/activity-row';
import { formatCompactRelativeTimestamp } from '@app/features/entity/utils/timestamp';
import { InsightSection } from '@app/features/scheduling/components/insights-panel';
import { t } from '@app/lib/i18n';
import { createUniqueId, For, Show } from 'solid-js';
import { RunStatusDot, runStatusDetail } from '../../components/run-status';
import { useRunsContext } from '../../context/runs-context';
import { type Run, runStatus, runTitle } from '../../core/run';

/**
 * The person's Runs that hold a place in their Queue, newest first, each
 * opened from its row: those waiting on them at a Gate, then those yet to
 * reach their first Gate, which hold the place they will ask for. Nothing
 * shows while no Run holds one.
 */
export function WaitingRuns(props: { onOpenRun: (runId: string) => void }) {
  const queue = useRunsContext().waitingRuns();
  const waiting = () => queue.data()?.waiting ?? [];
  const starting = () => queue.data()?.starting ?? [];
  const startingId = createUniqueId();

  return (
    <Show when={waiting().length > 0 || starting().length > 0}>
      <InsightSection
        title={t('factoryRuns.waitingOnYou')}
        description={t('factoryRuns.waitingOnYouHint')}
      >
        <div class="flex flex-col gap-3 pb-4">
          <Show when={waiting().length > 0}>
            <RunRows runs={waiting()} onOpenRun={props.onOpenRun} />
          </Show>
          <Show when={starting().length > 0}>
            <section aria-labelledby={startingId} class="flex flex-col gap-1">
              <h3 id={startingId} class="text-xs font-medium text-ink-muted">
                {t('factoryRuns.holdingAPlace')}
              </h3>
              <p class="text-xs text-ink-muted">
                {t('factoryRuns.holdingAPlaceHint')}
              </p>
              <RunRows runs={starting()} onOpenRun={props.onOpenRun} />
            </section>
          </Show>
        </div>
      </InsightSection>
    </Show>
  );
}

/** A row for each Run, which opens it. */
function RunRows(props: { runs: Run[]; onOpenRun: (runId: string) => void }) {
  return (
    <div class="-mx-2 flex flex-col">
      <For each={props.runs}>
        {(run) => (
          <ActivityRow
            name={runTitle(run)}
            icon={<RunStatusDot status={runStatus(run)} />}
            latest={runStatusDetail(runStatus(run))}
            time={formatCompactRelativeTimestamp(run.createdAt)}
            unseen={false}
            steps={[]}
            expanded={false}
            onToggle={() => {}}
            onOpen={() => props.onOpenRun(run.id)}
          />
        )}
      </For>
    </div>
  );
}
