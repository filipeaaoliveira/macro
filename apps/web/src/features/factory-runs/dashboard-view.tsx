import { ListHeader } from '@app/components/view-shell/ListHeader';
import { ViewSidebar } from '@app/components/view-shell/ViewSidebar';
import { t } from '@app/lib/i18n';
import { HeaderActionGroup } from '@components/app/HeaderActionGroup';
import ChartBarIcon from '@icon/system/regular/chart-bar.svg';
import { Button, Tabs } from '@ui';
import {
  type Accessor,
  createMemo,
  createSignal,
  Match,
  onCleanup,
  Show,
  Switch,
} from 'solid-js';
import { FactoryBoardView } from './board-view';
import { useBoardContext } from './context/board-context';
import { type RunsSource, useRunsContext } from './context/runs-context';
import type { BoardRun, BoardScope, BoardState } from './core/board';
import { liveNumbers } from './core/live';
import type { DeliveryInsight } from './core/quality';
import type { Run } from './core/run';
import { RunView } from './run-view';
import { DashboardView, notifyDashboardAnswer } from './views/dashboard';

/** How often elapsed times tick. */
const TICK_MS = 1000;

type Mode = 'live' | 'all';

/** Board Runs in any of `states`, joined, newest first. */
function joined(sources: RunsSource<BoardRun[]>[]): RunsSource<BoardRun[]> {
  return {
    state: () => {
      const states = sources.map((source) => source.state());
      if (states.includes('pending')) return 'pending';
      return states.includes('error') ? 'error' : 'ready';
    },
    data: () => {
      const lists = sources.map((source) => source.data());
      if (lists.some((list) => list === undefined)) return undefined;
      return lists
        .flatMap((list) => list ?? [])
        .sort((a, b) => b.run.createdAt.localeCompare(a.run.createdAt));
    },
    retry: () => {
      for (const source of sources) source.retry();
    },
  };
}

/**
 * The factory's home: the live dashboard, and every Run as the board lists
 * them a click away. A Run opens in a pane beside it.
 */
export function FactoryDashboard(props: {
  onOpenRun?: (runId: string) => void;
  onOpenInsights?: () => void;
  /** The entries of the team's Definition's Flow, by its name. */
  flowEntries?: (flow: string) => readonly string[] | undefined;
  /** How the last week's Runs delivered, for autonomy and cost. */
  delivery?: () => DeliveryInsight | undefined;
}) {
  const board = useBoardContext();
  const runs = useRunsContext();
  const [mode, setMode] = createSignal<Mode>('live');
  const [scope, setScope] = createSignal<BoardScope>('mine');
  const [openRunId, setOpenRunId] = createSignal<string>();
  const [now, setNow] = createSignal(Date.now());
  const clock = setInterval(() => setNow(Date.now()), TICK_MS);
  onCleanup(() => clearInterval(clock));

  const inState =
    (
      state: BoardState
    ): Accessor<{
      scope: BoardScope;
      state: BoardState;
    }> =>
    () => ({ scope: scope(), state });
  const active = joined([
    board.board(inState('running')),
    board.board(inState('held')),
  ]);
  const waiting = board.board(inState('waiting'));
  const recent = board.board(() => ({ scope: scope() }));
  const waitingRuns = runs.waitingRuns();
  const queueLimit = board.queueLimit();
  const queue: RunsSource<{ held: Run[]; places: number }> = {
    ...waitingRuns,
    data: () => {
      const places = waitingRuns.data();
      if (!places) return undefined;
      const held = [...places.starting, ...places.waiting];
      return { held, places: held.length };
    },
  };
  const numbers = createMemo(() =>
    liveNumbers({
      active: active.data() ?? [],
      waiting: waiting.data()?.length ?? 0,
      recent: recent.data() ?? [],
      now: now(),
    })
  );
  const open = (runId: string) =>
    props.onOpenRun ? props.onOpenRun(runId) : setOpenRunId(runId);

  const modeTabs = () => (
    <Tabs
      aria-label={t('factoryDashboard.view')}
      size="md"
      options={[
        { value: 'live' as const, label: t('factoryDashboard.live') },
        { value: 'all' as const, label: t('factoryDashboard.allRuns') },
      ]}
      value={mode()}
      onChange={(value: Mode) => setMode(value)}
    />
  );

  return (
    <Switch>
      <Match when={mode() === 'all'}>
        <FactoryBoardView
          onOpenRun={props.onOpenRun}
          headerStart={modeTabs()}
        />
      </Match>
      <Match when={mode() === 'live'}>
        <div class="flex size-full min-h-0 flex-col">
          <ListHeader
            title={
              <ViewSidebar.Title caret={false}>
                {t('factoryBoard.title')}
              </ViewSidebar.Title>
            }
          >
            {modeTabs()}
            <Tabs
              aria-label={t('factoryBoard.scope')}
              size="md"
              options={[
                { value: 'mine' as const, label: t('factoryBoard.scopeMine') },
                { value: 'team' as const, label: t('factoryBoard.scopeTeam') },
              ]}
              value={scope()}
              onChange={(value: BoardScope) => setScope(value)}
            />
            <Show when={props.onOpenInsights}>
              {(openInsights) => (
                <HeaderActionGroup label={t('factoryBoard.actions')} size="md">
                  <Button
                    size="md"
                    variant="ghost"
                    onClick={() => openInsights()()}
                  >
                    <ChartBarIcon class="size-4" />
                    {t('factoryDashboard.insights')}
                  </Button>
                </HeaderActionGroup>
              )}
            </Show>
          </ListHeader>
          <div class="min-h-0 flex-1 overflow-y-auto pt-1">
            <DashboardView
              data={{
                active,
                waiting,
                recent,
                queue,
                queueLimit,
                numbers,
                delivery: props.delivery,
              }}
              viewerId={board.viewerId()}
              entries={props.flowEntries}
              now={now()}
              onOpenRun={open}
              onAnswer={async (input) =>
                notifyDashboardAnswer(
                  await runs.answerGate({
                    runId: input.runId,
                    step: input.step,
                    answer: input.answer,
                  })
                )
              }
            />
          </div>
          <Show when={openRunId()} keyed>
            {(runId) => (
              <RunView runId={runId} onClose={() => setOpenRunId(undefined)} />
            )}
          </Show>
        </div>
      </Match>
    </Switch>
  );
}
