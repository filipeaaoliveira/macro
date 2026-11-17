import { ShowFeatureFlag } from '@app/lib/analytics/posthog';
import { useOptionalSplitRouter } from '@app/lib/split-router';
import { PaneContext } from '@app/lib/split-router/solid/context';
import { factoryInsightsRoute } from '@app/routes/routes';
import { enableFactoryRuns } from '@core/constant/featureFlags';
import { useSettingsState } from '@core/constant/SettingsState';
import { fireAndForget } from '@core/util/fireAndForget';
import { queryClient } from '@queries/client';
import { useContext } from 'solid-js';
import { InsightsProvider } from './context/insights-context';
import { RunsProvider } from './context/runs-context';
import { RunInsightsView } from './insights-view';
import { createAppInsightsContext } from './queries/insights';
import { createAppRunsContext } from './queries/runs';
import { ControlFilesSettings } from './views/control-files-settings';
import { StandingRulesSettings } from './views/standing-rules-settings';
import { TaskRunsSection } from './views/task-runs-section';

/**
 * Opens Run insights in a new pane beside the one asking; none outside the
 * split router.
 */
function useOpenInsights(): (() => void) | undefined {
  const router = useOptionalSplitRouter();
  const pane = useContext(PaneContext);
  if (!router) return;

  return () =>
    fireAndForget(
      router.open(
        { route: factoryInsightsRoute },
        { newPane: true, source: pane?.pane() }
      ),
      'factory-runs'
    );
}

/** A task's Runs, as a section of its side panel. */
export function TaskRuns(props: { taskId: string; order: number }) {
  const openInsights = useOpenInsights();
  const { openSettings } = useSettingsState();

  return (
    <ShowFeatureFlag flag={enableFactoryRuns}>
      <RunsProvider value={createAppRunsContext(queryClient)}>
        <TaskRunsSection
          taskId={props.taskId}
          order={props.order}
          onOpenInsights={openInsights}
          onConnectGitHub={() => openSettings('Connected')}
        />
      </RunsProvider>
    </ShowFeatureFlag>
  );
}

/**
 * The team's Control Files, as the body of a team settings section: the
 * patterns its Runs' changes are checked against.
 */
export function TeamControlFiles() {
  return <ControlFilesSettings />;
}

/**
 * The team's Standing Rules, as the body of a team settings section: which
 * kinds of Gate the factory answers on its own.
 */
export function TeamStandingRules() {
  return <StandingRulesSettings />;
}

/** How the person's Runs went, as a page of its own. */
export function FactoryInsights() {
  return (
    <ShowFeatureFlag flag={enableFactoryRuns}>
      <RunsProvider value={createAppRunsContext(queryClient)}>
        <InsightsProvider value={createAppInsightsContext()}>
          <RunInsightsView
            timeZone={Intl.DateTimeFormat().resolvedOptions().timeZone}
            now={() => new Date()}
          />
        </InsightsProvider>
      </RunsProvider>
    </ShowFeatureFlag>
  );
}
