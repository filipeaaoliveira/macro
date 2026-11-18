import { ActivityRow } from '@app/features/agent-session-activity/components/activity-row';
import { formatCompactRelativeTimestamp } from '@app/features/entity/utils/timestamp';
import { InsightSection } from '@app/features/scheduling/components/insights-panel';
import { t } from '@app/lib/i18n';
import { For, Show } from 'solid-js';
import { RunStatusDot, runStatusDetail } from '../../components/run-status';
import { useRunsContext } from '../../context/runs-context';
import { runStatus, runTitle } from '../../core/run';

/**
 * The person's Runs that wait on them at a Gate, newest first, each opened
 * to answer it. Nothing shows while none waits.
 */
export function WaitingRuns(props: { onOpenRun: (runId: string) => void }) {
  const runs = useRunsContext().waitingRuns();
  const waiting = () => runs.data() ?? [];

  return (
    <Show when={waiting().length > 0}>
      <InsightSection
        title={t('factoryRuns.waitingOnYou')}
        description={t('factoryRuns.waitingOnYouHint')}
      >
        <div class="-mx-2 flex flex-col pb-4">
          <For each={waiting()}>
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
      </InsightSection>
    </Show>
  );
}
