import { ShowFeatureFlag } from '@app/lib/analytics/posthog';
import { enableFactoryRuns } from '@core/constant/featureFlags';
import { queryClient } from '@queries/client';
import { RunsProvider } from './context/runs-context';
import { createAppRunsContext } from './queries/runs';
import { TaskRunsSection } from './views/task-runs-section';

/** A task's Runs, as a section of its side panel. */
export function TaskRuns(props: { taskId: string; order: number }) {
  return (
    <ShowFeatureFlag flag={enableFactoryRuns}>
      <RunsProvider value={createAppRunsContext(queryClient)}>
        <TaskRunsSection taskId={props.taskId} order={props.order} />
      </RunsProvider>
    </ShowFeatureFlag>
  );
}
