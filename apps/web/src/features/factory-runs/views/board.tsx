import { ListHeader } from '@app/components/view-shell/ListHeader';
import { SearchBar } from '@app/components/view-shell/SearchBar';
import { ViewSidebar } from '@app/components/view-shell/ViewSidebar';
import { formatCompactRelativeTimestamp } from '@app/features/entity/utils/timestamp';
import { SelectInput } from '@app/features/scheduling/components/fields';
import { SchedulingFilterSection } from '@app/features/scheduling/components/scheduling-filter-section';
import { SchedulingFilters } from '@app/features/scheduling/components/scheduling-filters';
import { t } from '@app/lib/i18n';
import { HeaderActionGroup } from '@components/app/HeaderActionGroup';
import { LoadingSpinner } from '@core/component/LoadingSpinner';
import { Button, Tabs } from '@ui';
import { createSignal, For, Match, onCleanup, Show, Switch } from 'solid-js';
import { match } from 'ts-pattern';
import { BoardRunRow } from '../components/board-row';
import type { BoardProject, BoardSource } from '../context/board-context';
import type { RunsSource } from '../context/runs-context';
import {
  BOARD_STATES,
  type BoardRun,
  type BoardScope,
  type BoardState,
  repositoryName,
} from '../core/board';
import { flowWordLabel } from '../core/flow-words';
import type { BoardViewState } from '../primitives/board-state';

/** How often row ages ("5m") move on. */
const CLOCK_MS = 30_000;

const ALL = 'all';

export function boardStateLabel(state: BoardState): string {
  return match(state)
    .with('running', () => t('factoryBoard.stateRunning'))
    .with('waiting', () => t('factoryBoard.stateWaiting'))
    .with('held', () => t('factoryBoard.stateHeld'))
    .with('stopped', () => t('factoryBoard.stateStopped'))
    .with('finished', () => t('factoryBoard.stateFinished'))
    .exhaustive();
}

/** The Flow, repository and Project, as Scheduling's filters pick them. */
function BoardFilterMenu(props: {
  state: BoardViewState;
  flows: string[];
  repositories: string[];
  projects: BoardProject[];
}) {
  const flowOptions = () => [
    { value: ALL, label: t('factoryBoard.allFlows') },
    ...props.flows.map((flow) => ({ value: flow, label: flowWordLabel(flow) })),
  ];
  const repositoryOptions = () => [
    { value: ALL, label: t('factoryBoard.allRepositories') },
    ...props.repositories.map((url) => ({
      value: url,
      label: repositoryName(url),
    })),
  ];
  const projectOptions = () => [
    { value: ALL, label: t('factoryBoard.allProjects') },
    ...props.projects.map((project) => ({
      value: project.id,
      label: project.name,
    })),
  ];
  const pick = (set: (value: string | undefined) => void) => (value: string) =>
    set(value === ALL ? undefined : value);
  const flow = () => props.state.flow() ?? ALL;
  const repository = () => props.state.repoUrl() ?? ALL;
  const project = () => props.state.projectId() ?? ALL;

  return (
    <SchedulingFilters
      grouped
      size="md"
      title={t('factoryBoard.filters')}
      activeCount={
        Number(props.state.flow() !== undefined) +
        Number(props.state.repoUrl() !== undefined) +
        Number(props.state.projectId() !== undefined)
      }
      onClear={props.state.clearFilters}
      mobileChildren={
        <>
          <SchedulingFilterSection
            value="flow"
            label={t('factoryBoard.flow')}
            selected={flow()}
            options={flowOptions()}
            onChange={pick(props.state.setFlow)}
          />
          <SchedulingFilterSection
            value="repository"
            label={t('factoryBoard.repository')}
            selected={repository()}
            options={repositoryOptions()}
            onChange={pick(props.state.setRepoUrl)}
          />
          <SchedulingFilterSection
            value="project"
            label={t('factoryBoard.project')}
            selected={project()}
            options={projectOptions()}
            onChange={pick(props.state.setProjectId)}
          />
        </>
      }
    >
      <SelectInput
        label={t('factoryBoard.flow')}
        class="w-full text-sm"
        value={flow()}
        options={flowOptions()}
        onChange={pick(props.state.setFlow)}
      />
      <SelectInput
        label={t('factoryBoard.repository')}
        class="w-full text-sm"
        value={repository()}
        options={repositoryOptions()}
        onChange={pick(props.state.setRepoUrl)}
      />
      <SelectInput
        label={t('factoryBoard.project')}
        class="w-full text-sm"
        value={project()}
        options={projectOptions()}
        onChange={pick(props.state.setProjectId)}
      />
    </SchedulingFilters>
  );
}

/**
 * The factory's capacity for the person: the places their Queue holds
 * against its limit, and the Runs waiting on them.
 */
function BoardCapacity(props: {
  held: number;
  waiting: number;
  limit: number | null | undefined;
}) {
  const queue = () =>
    props.limit === null || props.limit === undefined
      ? t('factoryBoard.queueUnlimited', { count: props.held })
      : t('factoryBoard.queueHeld', { count: props.held, limit: props.limit });

  return (
    <dl class="flex flex-wrap gap-x-6 gap-y-1 px-4 pt-3 text-xs">
      <div class="flex gap-1.5">
        <dt class="text-ink-muted">{t('factoryBoard.queue')}</dt>
        <dd class="font-medium tabular-nums text-ink" data-board-queue>
          {queue()}
        </dd>
      </div>
      <div class="flex gap-1.5">
        <dt class="text-ink-muted">{t('factoryBoard.waitingOnYou')}</dt>
        <dd class="font-medium tabular-nums text-ink" data-board-waiting>
          {props.waiting}
        </dd>
      </div>
    </dl>
  );
}

/** The loaded Runs grouped by where they are, in the board's order. */
function groupByState(runs: BoardRun[]) {
  return BOARD_STATES.map((state) => ({
    state,
    runs: runs.filter((listed) => listed.state === state),
  })).filter((group) => group.runs.length > 0);
}

/**
 * The factory's board: whose Runs (the person's or their team's), where
 * they are, the filters and a search by task, the person's capacity, and
 * the Runs grouped by state, a page at a time.
 */
export function BoardView(props: {
  state: BoardViewState;
  onOpenRun: (runId: string) => void;
  source: BoardSource;
  flows: string[];
  repositories: string[];
  projects: BoardProject[];
  queue: RunsSource<{ held: number; waiting: number }>;
  queueLimit: RunsSource<number | null>;
}) {
  const [now, setNow] = createSignal(Date.now());
  const clock = setInterval(() => setNow(Date.now()), CLOCK_MS);
  onCleanup(() => clearInterval(clock));
  const age = (time: string) => {
    now();
    return formatCompactRelativeTimestamp(time);
  };
  const scopeOptions = () => [
    { value: 'mine' as const, label: t('factoryBoard.scopeMine') },
    { value: 'team' as const, label: t('factoryBoard.scopeTeam') },
  ];
  const stateOptions = () => [
    { value: ALL, label: t('factoryBoard.allStates') },
    ...BOARD_STATES.map((state) => ({
      value: state,
      label: boardStateLabel(state),
    })),
  ];
  const groups = () => groupByState(props.source.data() ?? []);

  return (
    <div class="flex size-full min-h-0 flex-col">
      <ListHeader
        title={
          <ViewSidebar.Title caret={false}>
            {t('factoryBoard.title')}
          </ViewSidebar.Title>
        }
      >
        <Tabs
          aria-label={t('factoryBoard.scope')}
          size="md"
          options={scopeOptions()}
          value={props.state.scope()}
          onChange={(scope: BoardScope) => props.state.setScope(scope)}
        />
        <HeaderActionGroup label={t('factoryBoard.actions')} size="md">
          <BoardFilterMenu
            state={props.state}
            flows={props.flows}
            repositories={props.repositories}
            projects={props.projects}
          />
        </HeaderActionGroup>
      </ListHeader>
      <Show when={props.queue.data()}>
        {(queue) => (
          <BoardCapacity
            held={queue().held}
            waiting={queue().waiting}
            limit={props.queueLimit.data()}
          />
        )}
      </Show>
      <div class="flex flex-wrap items-center gap-2 px-4 pt-3">
        <Tabs
          aria-label={t('factoryBoard.states')}
          size="sm"
          wrap
          options={stateOptions()}
          value={props.state.state() ?? ALL}
          onChange={(value: string) =>
            props.state.setState(BOARD_STATES.find((state) => state === value))
          }
        />
        <SearchBar
          label={t('factoryBoard.search')}
          placeholder={t('factoryBoard.search')}
          size="sm"
          class="min-w-40 flex-1"
          value={props.state.searchText()}
          onValueChange={props.state.setSearchText}
        />
      </div>
      <div class="min-h-0 flex-1 overflow-y-auto px-4 pb-8 pt-3">
        <Switch>
          <Match when={props.source.state() === 'pending'}>
            <div class="flex justify-center py-8">
              <LoadingSpinner />
            </div>
          </Match>
          <Match when={props.source.state() === 'error'}>
            <div class="flex items-center gap-2">
              <p class="text-sm text-ink-muted">
                {t('factoryBoard.loadFailed')}
              </p>
              <Button
                variant="ghost"
                size="xs"
                onClick={() => props.source.retry()}
              >
                {t('common.retry')}
              </Button>
            </div>
          </Match>
          <Match when={groups().length === 0}>
            <div class="flex flex-col gap-1 py-8 text-center">
              <p class="text-sm font-medium text-ink">
                {t('factoryBoard.empty')}
              </p>
              <p class="text-xs text-ink-muted">
                {props.state.scope() === 'team'
                  ? t('factoryBoard.emptyHintTeam')
                  : t('factoryBoard.emptyHint')}
              </p>
            </div>
          </Match>
          <Match when={groups().length > 0}>
            <div class="flex flex-col gap-4">
              <For each={groups()}>
                {(group) => (
                  <section
                    aria-label={boardStateLabel(group.state)}
                    class="flex flex-col gap-1"
                  >
                    <h2 class="flex items-center gap-1.5 px-2 text-xs font-medium text-ink-muted">
                      {boardStateLabel(group.state)}
                      <span class="tabular-nums text-ink-subtle">
                        {props.source.hasMore()
                          ? t('factoryBoard.loadedCount', {
                              count: group.runs.length,
                            })
                          : group.runs.length}
                      </span>
                    </h2>
                    <ul class="-mx-2 flex flex-col">
                      <For each={group.runs}>
                        {(listed) => (
                          <BoardRunRow
                            listed={listed}
                            age={age}
                            onOpen={props.onOpenRun}
                          />
                        )}
                      </For>
                    </ul>
                  </section>
                )}
              </For>
              <Show when={props.source.stoppedEarly()}>
                <p class="px-2 text-xs text-ink-muted" data-board-stopped>
                  {t('factoryBoard.stoppedEarly')}
                </p>
              </Show>
              <Show when={props.source.hasMore()}>
                <div class="flex justify-center">
                  <Button
                    variant="ghost"
                    size="sm"
                    disabled={props.source.loadingMore()}
                    onClick={() => props.source.loadMore()}
                  >
                    {t('factoryBoard.loadMore')}
                  </Button>
                </div>
              </Show>
            </div>
          </Match>
        </Switch>
      </div>
    </div>
  );
}
