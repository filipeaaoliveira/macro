import { ShowFeatureFlag } from '@app/lib/analytics/posthog';
import { useOptionalSplitRouter } from '@app/lib/split-router';
import { PaneContext } from '@app/lib/split-router/solid/context';
import { factoryInsightsRoute, factoryRunRoute } from '@app/routes/routes';
import { enableFactoryRuns } from '@core/constant/featureFlags';
import { useSettingsState } from '@core/constant/SettingsState';
import { useUserId } from '@core/context/user';
import { fireAndForget } from '@core/util/fireAndForget';
import { queryClient } from '@queries/client';
import { useContext } from 'solid-js';
import { FactoryBoardView } from './board-view';
import { BoardProvider } from './context/board-context';
import { InsightsProvider } from './context/insights-context';
import { RunsProvider } from './context/runs-context';
import { RunInsightsView } from './insights-view';
import { createAppBoardContext } from './queries/board';
import { createAppInsightsContext } from './queries/insights';
import { createAppRunsContext } from './queries/runs';
import { RunPageView } from './run-view';
import { AutomationsSettings } from './views/automations-settings';
import { ControlFilesSettings } from './views/control-files-settings';
import { DefinitionSettings } from './views/definition-settings';
import { StandingRulesSettings } from './views/standing-rules-settings';
import { TaskRunsSection } from './views/task-runs-section';

/** The Runs context on the factory service, for the signed-in person. */
function appRunsContext() {
  return { ...createAppRunsContext(queryClient), viewerId: useUserId() };
}

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

/**
 * Opens a Run in a new pane beside the one asking; none outside the split
 * router.
 */
function useOpenRun(): ((runId: string) => void) | undefined {
  const router = useOptionalSplitRouter();
  const pane = useContext(PaneContext);
  if (!router) return;

  return (runId) =>
    fireAndForget(
      router.open(
        { route: factoryRunRoute, params: { runId } },
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
      <RunsProvider value={appRunsContext()}>
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
 * The team's Factory Definition, as the body of a team settings section:
 * the repository its Runs take their Flows, Agents and rules from.
 */
export function TeamDefinition() {
  return <DefinitionSettings />;
}

/**
 * The team's Standing Rules, as the body of a team settings section: which
 * kinds of Gate the factory answers on its own.
 */
export function TeamStandingRules() {
  return <StandingRulesSettings />;
}

/**
 * The team's Automations, as the body of a team settings section: what
 * starts a Run on its own, as the person who made it.
 */
export function TeamAutomations() {
  return <AutomationsSettings />;
}

/** How the person's Runs went, as a page of its own. */
export function FactoryInsights() {
  return (
    <ShowFeatureFlag flag={enableFactoryRuns}>
      <RunsProvider value={appRunsContext()}>
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

/** One Run as a page of its own, as the board and a Run's link open it. */
export function FactoryRun(props: { runId: string }) {
  return (
    <ShowFeatureFlag flag={enableFactoryRuns}>
      <RunsProvider value={appRunsContext()}>
        <RunPageView runId={props.runId} />
      </RunsProvider>
    </ShowFeatureFlag>
  );
}

/**
 * Every Run the person can see, theirs or their team's, as a page of its
 * own; a Run opens in a pane beside it.
 */
export function FactoryBoard() {
  const openRun = useOpenRun();

  return (
    <ShowFeatureFlag flag={enableFactoryRuns}>
      <RunsProvider value={appRunsContext()}>
        <BoardProvider value={createAppBoardContext(queryClient)}>
          <FactoryBoardView onOpenRun={openRun} />
        </BoardProvider>
      </RunsProvider>
    </ShowFeatureFlag>
  );
}
