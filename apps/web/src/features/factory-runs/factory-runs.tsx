import { ShowFeatureFlag, useFeatureFlag } from '@app/lib/analytics/posthog';
import { t } from '@app/lib/i18n';
import { useOptionalSplitRouter } from '@app/lib/split-router';
import { PaneContext } from '@app/lib/split-router/solid/context';
import { factoryInsightsRoute, factoryRunRoute } from '@app/routes/routes';
import type { PrMergeGuard } from '@block-pr/primitives/pr-merge-guard-context';
import { toast } from '@core/component/Toast/Toast';
import { enableFactoryRuns } from '@core/constant/featureFlags';
import { useSettingsState } from '@core/constant/SettingsState';
import { useUserId } from '@core/context/user';
import { fireAndForget } from '@core/util/fireAndForget';
import {
  normalizeUserNameQueryId,
  useUserNames,
} from '@queries/auth/user-names';
import { queryClient } from '@queries/client';
import {
  type Accessor,
  createMemo,
  createSignal,
  Show,
  useContext,
} from 'solid-js';
import { FactoryBoardView } from './board-view';
import {
  PullRequestRunMark,
  pullRequestMergeNote,
} from './components/pull-request-run-mark';
import { BoardProvider } from './context/board-context';
import { InsightsProvider } from './context/insights-context';
import { RunsProvider } from './context/runs-context';
import { type PullRequestRun, pullRequestMerge } from './core/pull-request-run';
import { RunInsightsView } from './insights-view';
import { createAppBoardContext } from './queries/board';
import { createAppInsightsContext } from './queries/insights';
import { usePullRequestRuns } from './queries/pull-request-runs';
import { createAppRunsContext } from './queries/runs';
import { RunPageView, RunView } from './run-view';
import { AutomationsSettings } from './views/automations-settings';
import { ControlFilesSettings } from './views/control-files-settings';
import { DefinitionSettings } from './views/definition-settings';
import { PullRequestRunSection } from './views/pull-request-run-section';
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
        <InsightsProvider value={createAppInsightsContext(queryClient)}>
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

/**
 * Where a pull request's side panel shows its Run's section: above the
 * Checks section (20), as what decides the merge.
 */
export const PULL_REQUEST_RUN_SECTION_ORDER = 15;

/**
 * The factory's part of a pull request's views (Reviews, its detail, its
 * merge button), for the pull requests at `urls`: the mark of each one a
 * Run the person can see opened, its Run's section beside it, and merges
 * that go through the Run's merge Gate. Until the factory has said whether
 * a Run opened one, it does not merge. Pull requests no Run opened get
 * nothing and merge as before.
 */
export type FactoryReviews = PrMergeGuard;

export function useFactoryReviews(
  urls: Accessor<readonly string[]>
): FactoryReviews {
  const flag = useFeatureFlag(enableFactoryRuns);
  // Until the flag is read, whether a Run guards a pull request isn't known.
  const runs = usePullRequestRuns(urls, () =>
    flag().loading ? undefined : flag().enabled
  );
  const viewerId = useUserId();
  const openRun = useOpenRun();
  const [gateRunId, setGateRunId] = createSignal<string>();
  const foundFor = (url: string) => {
    const lookup = runs.lookup(url);
    return lookup.kind === 'run' ? lookup.found : undefined;
  };
  const ownerIds = createMemo(() =>
    urls().flatMap((url) => foundFor(url)?.run.ownerId ?? [])
  );
  const names = useUserNames({ userIds: ownerIds });

  const ownerName = (found: PullRequestRun | undefined) => {
    const ownerId = found?.run.ownerId;
    if (!ownerId) return undefined;
    if (ownerId === viewerId()) return t('factoryBoard.you');
    const id = normalizeUserNameQueryId(ownerId);
    const name = names().find(
      (entry) => entry.name && normalizeUserNameQueryId(entry.name.id) === id
    )?.name;
    const full = [name?.first_name, name?.last_name].filter(Boolean).join(' ');
    return full || undefined;
  };
  const mergeOf = (url: string) =>
    pullRequestMerge(runs.lookup(url), viewerId());
  const refusal = (url: string) => {
    const merge = mergeOf(url);
    if (merge.kind !== 'refused') return undefined;
    return pullRequestMergeNote(merge, ownerName(foundFor(url)));
  };

  return {
    mark: (url) => (
      <Show when={foundFor(url)}>
        {(found) => (
          <PullRequestRunMark
            found={found()}
            merge={mergeOf(url)}
            ownerName={ownerName(found())}
            onOpenRun={openRun}
          />
        )}
      </Show>
    ),
    section: (url) => (
      <Show when={foundFor(url)}>
        {(found) => (
          <PullRequestRunSection
            found={found()}
            merge={mergeOf(url)}
            ownerName={ownerName(found())}
            order={PULL_REQUEST_RUN_SECTION_ORDER}
            onOpenRun={openRun}
            onAnswerGate={setGateRunId}
          />
        )}
      </Show>
    ),
    merge: (url) => {
      const merge = mergeOf(url);
      if (merge.kind === 'github') return false;
      if (merge.kind === 'gate') {
        setGateRunId(merge.runId);
        return true;
      }
      const why = refusal(url);
      if (why) toast.failure(why);
      return true;
    },
    mergeRefusal: refusal,
    mergeWarning: (url) => {
      const merge = mergeOf(url);
      if (merge.kind !== 'github' || !merge.stopped) return undefined;
      return pullRequestMergeNote(merge, undefined);
    },
    gate: () => (
      <Show when={gateRunId()} keyed>
        {(runId) => (
          <RunsProvider value={appRunsContext()}>
            <RunView runId={runId} onClose={() => setGateRunId(undefined)} />
          </RunsProvider>
        )}
      </Show>
    ),
  };
}
