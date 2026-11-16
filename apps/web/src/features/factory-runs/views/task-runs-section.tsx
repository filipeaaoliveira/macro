import { formatCompactRelativeTimestamp } from '@app/features/entity/utils/timestamp';
import { t } from '@app/lib/i18n';
import { SidePanel, useSidePanel } from '@components/app/side-panel';
import { isTouchDevice } from '@core/mobile/isTouchDevice';
import PlusIcon from '@icon/system/regular/plus.svg';
import { Button } from '@ui';
import { createSignal, For, Match, Show, Switch } from 'solid-js';
import { RunRow } from '../components/run-row';
import { StartRunDialog } from '../components/start-run-dialog';
import { useRunsContext } from '../context/runs-context';
import { runStatus, runTitle } from '../core/run';
import { RunDialog } from './run-dialog';

/** Starts a Run on the task with the Flows the factory offers. */
function StartRun(props: {
  taskId: string;
  onClose: () => void;
  onStarted: (runId: string) => void;
}) {
  const context = useRunsContext();
  const flows = context.flows();

  return (
    <StartRunDialog
      flows={flows.data()}
      flowsState={flows.state()}
      onRetryFlows={flows.retry}
      onOpenChange={(open) => !open && props.onClose()}
      onStart={async (flow, request) => {
        const result = await context.startRun({
          taskId: props.taskId,
          flow,
          request,
        });
        if (!result.ok) return false;
        props.onStarted(result.value.id);
        return true;
      }}
    />
  );
}

/** A task's Runs in its side panel, with New run and each Run's dialog. */
export function TaskRunsSection(props: { taskId: string; order: number }) {
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
        }
        defaultOpen
        order={props.order}
      >
        <Switch>
          <Match when={runs.state() === 'pending'}>
            <div class="text-xs text-ink-placeholder" role="status">
              {t('factoryRuns.loading')}
            </div>
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
                      <RunRow
                        title={runTitle(run)}
                        status={runStatus(run)}
                        time={formatCompactRelativeTimestamp(run.createdAt)}
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
          onClose={() => setStarting(false)}
          onStarted={setOpenRunId}
        />
      </Show>
      <Show when={openRunId()} keyed>
        {(runId) => (
          <RunDialog runId={runId} onClose={() => setOpenRunId(undefined)} />
        )}
      </Show>
    </>
  );
}
