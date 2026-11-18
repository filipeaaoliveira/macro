import { ListSkeleton } from '@app/components/view-shell/ListSkeleton';
import { ActivityRow } from '@app/features/agent-session-activity/components/activity-row';
import { formatCompactRelativeTimestamp } from '@app/features/entity/utils/timestamp';
import { t } from '@app/lib/i18n';
import { SidePanel, useSidePanel } from '@components/app/side-panel';
import { isTouchDevice } from '@core/mobile/isTouchDevice';
import ChartBarIcon from '@icon/system/regular/chart-bar.svg';
import PlusIcon from '@icon/system/regular/plus.svg';
import { Button } from '@ui';
import {
  createSignal,
  For,
  Index,
  Match,
  onCleanup,
  Show,
  Switch,
} from 'solid-js';
import { RunStatusDot, runStatusDetail } from '../components/run-status';
import { StartRunDialog } from '../components/start-run-dialog';
import { useRunsContext } from '../context/runs-context';
import { runStatus, runTitle } from '../core/run';
import { RunView } from '../run-view';

/** Starts a Run on the task with the Flows the factory offers. */
function StartRun(props: {
  taskId: string;
  onConnectGitHub?: () => void;
  onOpenQueue?: () => void;
  onClose: () => void;
  onStarted: (runId: string) => void;
}) {
  const context = useRunsContext();
  const flows = context.flows();
  // Read only when a Flow on offer works on a repository.
  const repositories = context.repositories(
    () => flows.data()?.some((flow) => flow.repository) ?? false
  );

  return (
    <StartRunDialog
      flows={flows.data()}
      flowsState={flows.state()}
      onRetryFlows={flows.retry}
      repositories={repositories.data()}
      repositoriesState={repositories.state()}
      onRetryRepositories={repositories.retry}
      onConnectGitHub={props.onConnectGitHub}
      onOpenQueue={props.onOpenQueue}
      onOpenChange={(open) => !open && props.onClose()}
      onStart={async (choice) => {
        const result = await context.startRun({
          taskId: props.taskId,
          ...choice,
        });
        if (!result.ok) return result.reason;
        props.onStarted(result.value.id);
        return 'started';
      }}
    />
  );
}

// How often row times move on, as in the agent activity section.
const CLOCK_MS = 30_000;

/** Placeholder rows in the geometry of the activity rows they become. */
function RunRowsSkeleton() {
  return (
    <ListSkeleton.Root label={t('factoryRuns.loading')} class="-mx-2">
      <Index each={[0, 1]}>
        {() => (
          <div class="flex items-start gap-2 px-2 py-1.5 touch:min-h-11">
            <span class="flex h-4.5 items-center">
              <ListSkeleton.Bar class="size-1.5" />
            </span>
            <span class="flex min-w-0 flex-1 flex-col gap-1.5 pt-1">
              <ListSkeleton.Bar class="w-40 max-w-full" />
              <ListSkeleton.Bar class="h-2 w-24" />
            </span>
            <ListSkeleton.Bar class="mt-1 w-6" />
          </div>
        )}
      </Index>
    </ListSkeleton.Root>
  );
}

/**
 * A task's Runs in its side panel, with New run and each Run's dialog, and a
 * way to the person's Run insights when the host offers one.
 */
export function TaskRunsSection(props: {
  taskId: string;
  order: number;
  onOpenInsights?: () => void;
  /** Where to send someone who reaches no repository yet. */
  onConnectGitHub?: () => void;
}) {
  const context = useRunsContext();
  const runs = context.taskRuns(() => props.taskId);
  const sidePanel = useSidePanel();
  const [starting, setStarting] = createSignal(false);
  const [openRunId, setOpenRunId] = createSignal<string>();
  // On touch the panel is a sheet over the pane; it closes first, or it
  // would hold the dialog's touches.
  const leaveSheet = () => {
    if (isTouchDevice()) sidePanel?.setIsOpen(false);
  };
  const count = () => runs.data()?.length ?? 0;
  // Polling stops once every Run has ended; the clock keeps "5m" honest.
  const [now, setNow] = createSignal(Date.now());
  const clock = setInterval(() => setNow(Date.now()), CLOCK_MS);
  onCleanup(() => clearInterval(clock));
  const time = (value: string) => {
    now();
    return formatCompactRelativeTimestamp(value);
  };

  return (
    <>
      <SidePanel.Section
        id="factory-runs"
        title={
          <SidePanel.CountTitle
            label={t('factoryRuns.title')}
            count={count()}
          />
        }
        actions={
          <>
            <Show when={props.onOpenInsights}>
              {(openInsights) => (
                <Button
                  variant="ghost"
                  size="icon-xs"
                  aria-label={t('factoryInsights.title')}
                  tooltip={t('factoryInsights.title')}
                  onClick={() => {
                    leaveSheet();
                    openInsights()();
                  }}
                >
                  <ChartBarIcon class="size-3 touch:size-6 shrink-0 pointer-events-none" />
                </Button>
              )}
            </Show>
            <Button
              variant="ghost"
              size="xs"
              onClick={() => {
                leaveSheet();
                setStarting(true);
              }}
            >
              <PlusIcon class="size-3 touch:size-6 shrink-0 pointer-events-none" />
              {t('factoryRuns.newRun')}
            </Button>
          </>
        }
        defaultOpen
        order={props.order}
      >
        <Switch>
          <Match when={runs.state() === 'pending'}>
            <RunRowsSkeleton />
          </Match>
          <Match when={runs.state() === 'error'}>
            <div class="flex items-center gap-2 text-xs">
              <span class="text-ink-placeholder">
                {t('factoryRuns.loadFailed')}
              </span>
              <Button variant="ghost" size="xs" onClick={runs.retry}>
                {t('common.retry')}
              </Button>
            </div>
          </Match>
          <Match when={runs.data()}>
            {(list) => (
              <Show
                when={list().length > 0}
                fallback={
                  <div class="text-xs text-ink-placeholder">
                    {t('factoryRuns.empty')}
                  </div>
                }
              >
                <div class="-mx-2 flex flex-col">
                  <For each={list()}>
                    {(run) => (
                      <ActivityRow
                        name={runTitle(run)}
                        icon={<RunStatusDot status={runStatus(run)} />}
                        latest={runStatusDetail(runStatus(run))}
                        time={time(run.createdAt)}
                        unseen={false}
                        steps={[]}
                        expanded={false}
                        onToggle={() => {}}
                        onOpen={() => {
                          leaveSheet();
                          setOpenRunId(run.id);
                        }}
                      />
                    )}
                  </For>
                </div>
              </Show>
            )}
          </Match>
        </Switch>
      </SidePanel.Section>
      <Show when={starting()}>
        <StartRun
          taskId={props.taskId}
          onConnectGitHub={props.onConnectGitHub}
          onOpenQueue={
            props.onOpenInsights
              ? () => {
                  setStarting(false);
                  props.onOpenInsights?.();
                }
              : undefined
          }
          onClose={() => setStarting(false)}
          onStarted={setOpenRunId}
        />
      </Show>
      <Show when={openRunId()} keyed>
        {(runId) => (
          <RunView runId={runId} onClose={() => setOpenRunId(undefined)} />
        )}
      </Show>
    </>
  );
}
