import { ListHeader } from '@app/components/view-shell/ListHeader';
import { ViewSidebar } from '@app/components/view-shell/ViewSidebar';
import { t } from '@app/lib/i18n';
import { LoadingSpinner } from '@core/component/LoadingSpinner';
import { Match, Show, Switch } from 'solid-js';
import { RunLiveHero } from '../components/run-live-hero';
import { runTitle } from '../core/run';
import { createFocusRescue } from '../primitives/focus-rescue';
import type { RunController } from '../primitives/run-controller';
import { RunGate } from './run-dialog';
import {
  RunCancel,
  RunContent,
  RunLoadFailed,
  RunRetry,
} from './shared/run-parts';
import { formatRunTime } from './shared/run-time';

/**
 * The Run as a pane of its own, as the board and a Run's link open it: its
 * Steps scroll, and, for its owner, the Gate it waits on and Cancel run, or
 * Retry run once it ended so, stay pinned under them, as in the dialog.
 */
export function RunPane(props: {
  controller: RunController;
  /** The entries of the team's Definition's Flow, by its name. */
  flowEntries?: (flow: string) => readonly string[] | undefined;
}) {
  let content: HTMLDivElement | undefined;
  let status: HTMLElement | undefined;
  const source = props.controller.source;
  createFocusRescue(
    props.controller,
    () => content,
    () => status
  );
  const title = () => {
    const run = source.data();
    return run ? runTitle(run) : t('factoryRuns.runDetails');
  };

  return (
    <div
      ref={(element) => {
        content = element;
      }}
      class="flex size-full min-h-0 flex-col"
    >
      <ListHeader
        title={<ViewSidebar.Title caret={false}>{title()}</ViewSidebar.Title>}
      />
      <Switch>
        <Match when={source.state() === 'pending'}>
          <div class="flex justify-center py-8">
            <LoadingSpinner />
          </div>
        </Match>
        <Match when={source.state() === 'error'}>
          <div class="px-4 pt-3">
            <RunLoadFailed onRetry={source.retry} />
          </div>
        </Match>
        <Match when={source.data()}>
          {(run) => (
            <>
              <div class="min-h-0 flex-1 overflow-y-auto px-4 pb-6 pt-3 sm:px-6">
                <RunLiveHero
                  run={run()}
                  entries={
                    run().flow
                      ? props.flowEntries?.(run().flow ?? '')
                      : undefined
                  }
                  onWatch={
                    props.controller.owned()
                      ? props.controller.openTerminal
                      : undefined
                  }
                  actions={
                    <>
                      <Show when={props.controller.owned() && !run().end}>
                        <RunCancel controller={props.controller} />
                      </Show>
                      <Show when={props.controller.retriable()}>
                        <RunRetry controller={props.controller} />
                      </Show>
                    </>
                  }
                />
                <RunContent
                  stepsFirst
                  controller={props.controller}
                  run={run()}
                  time={formatRunTime}
                  statusRef={(element) => {
                    status = element;
                  }}
                />
              </div>
              <Show
                when={props.controller.owned() && props.controller.waiting()}
              >
                <div class="shrink-0 border-t border-edge-divider px-4 pt-3">
                  <RunGate controller={props.controller} />
                </div>
              </Show>
            </>
          )}
        </Match>
      </Switch>
    </div>
  );
}
