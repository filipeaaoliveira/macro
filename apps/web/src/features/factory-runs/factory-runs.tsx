import { ShowFeatureFlag, useFeatureFlag } from '@app/lib/analytics/posthog';
import { t } from '@app/lib/i18n';
import { useOptionalSplitRouter } from '@app/lib/split-router';
import { PaneContext } from '@app/lib/split-router/solid/context';
import { factoryInsightsRoute, factoryRunRoute } from '@app/routes/routes';
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
  type JSX,
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

/**
 * The factory's part of Macro's Reviews page, for the pull requests at
 * `urls`: the mark of each one a Run the person can see opened, its Run's
 * section beside the pull request, and merges that go through the Run's
 * merge Gate. Pull requests no Run opened get nothing and merge as before.
 */
export type FactoryReviews = {
  /** The mark of the Run that opened the pull request at `url`, if one did. */
  mark: (url: string) => JSX.Element;
  /** Its Run as a section of the pull request's side panel, if one did. */
  section: (url: string, order: number) => JSX.Element;
  /**
   * Takes a merge of the pull request at `url` when a Run guards it, and
   * says so: its owner answers the Run's merge Gate, and anyone else is
   * told why it doesn't merge. False when no Run guards it, so GitHub
   * merges it as any pull request.
   */
  merge: (url: string) => boolean;
  /** Why merging the pull request at `url` does nothing now, if it doesn't. */
  mergeRefusal: (url: string) => string | undefined;
  /** The Run's merge Gate, once a merge opens it. */
  gate: () => JSX.Element;
};

export function useFactoryReviews(
  urls: Accessor<readonly string[]>
): FactoryReviews {
  const flag = useFeatureFlag(enableFactoryRuns);
  const runs = usePullRequestRuns(urls, () => flag().enabled);
  const viewerId = useUserId();
  const openRun = useOpenRun();
  const [gateRunId, setGateRunId] = createSignal<string>();
  const ownerIds = createMemo(() =>
    urls().flatMap((url) => runs.runFor(url)?.run.ownerId ?? [])
  );
  const names = useUserNames({ userIds: ownerIds });

  const ownerName = (found: PullRequestRun) => {
    const ownerId = found.run.ownerId;
    if (ownerId && ownerId === viewerId()) return t('factoryBoard.you');
    const id = normalizeUserNameQueryId(ownerId);
    const name = names().find(
      (entry) => entry.name && normalizeUserNameQueryId(entry.name.id) === id
    )?.name;
    const full = [name?.first_name, name?.last_name].filter(Boolean).join(' ');
    return full || undefined;
  };
  const mergeOf = (url: string) =>
    pullRequestMerge(runs.runFor(url), viewerId());
  const refusal = (url: string) => {
    const found = runs.runFor(url);
    const merge = mergeOf(url);
    if (!found || merge.kind !== 'refused') return undefined;
    return pullRequestMergeNote(merge, ownerName(found));
  };

  return {
    mark: (url) => (
      <Show when={runs.runFor(url)}>
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
    section: (url, order) => (
      <Show when={runs.runFor(url)}>
        {(found) => (
          <PullRequestRunSection
            found={found()}
            merge={mergeOf(url)}
            ownerName={ownerName(found())}
            order={order}
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
