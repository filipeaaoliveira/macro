import { t } from '@app/lib/i18n';
import { MobileDrawer } from '@components/app/mobile/MobileDrawer';
import { LoadingSpinner } from '@core/component/LoadingSpinner';
import { createSignal, Match, onCleanup, Show, Switch } from 'solid-js';
import { runTitle } from '../../core/run';
import { createFocusRescue } from '../../primitives/focus-rescue';
import type { RunController } from '../../primitives/run-controller';
import {
  RunCancel,
  RunContent,
  RunGate,
  RunLoadFailed,
} from '../shared/run-parts';
import { formatRunTime } from '../shared/run-time';

/** Slide-out length; keep at least MobileDrawer's transition. */
const CLOSE_MS = 250;

/**
 * The Run on a phone: a sheet whose pinned chrome, between the handle and
 * the scrolling Steps, holds the Gate it waits on and Cancel run.
 */
export function MobileRunSheet(props: {
  controller: RunController;
  onClose: () => void;
}) {
  const [open, setOpen] = createSignal(true);
  let content: HTMLDivElement | undefined;
  let status: HTMLElement | undefined;
  let closeTimer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => clearTimeout(closeTimer));
  const source = props.controller.source;
  createFocusRescue(
    props.controller,
    () => content,
    () => status
  );

  const close = () => {
    if (!open()) return;
    setOpen(false);
    closeTimer = setTimeout(props.onClose, CLOSE_MS);
  };

  const title = () => {
    const run = source.data();
    return run ? runTitle(run) : t('factoryRuns.runDetails');
  };

  return (
    <MobileDrawer
      side="bottom"
      open={open()}
      onOpenChange={(next) => !next && close()}
    >
      <MobileDrawer.Portal>
        <MobileDrawer.Overlay />
        <MobileDrawer.Content
          ref={(element: HTMLDivElement) => {
            content = element;
          }}
        >
          <MobileDrawer.Handle class="pb-1" />
          <MobileDrawer.Title class="shrink-0 truncate px-5 pb-3 text-lg font-semibold text-ink">
            {title()}
          </MobileDrawer.Title>
          <Show when={source.data()}>
            {(run) => (
              <>
                <Show when={props.controller.waiting()}>
                  <div class="shrink-0 px-4 pb-3">
                    <RunGate controller={props.controller} />
                  </div>
                </Show>
                <Show when={!run().end}>
                  <div class="flex shrink-0 justify-end px-4 pb-3">
                    <RunCancel controller={props.controller} />
                  </div>
                </Show>
              </>
            )}
          </Show>
          <MobileDrawer.ScrollBody>
            <div class="flex flex-col gap-5 border-t border-edge-divider px-5 pt-4 pb-6">
              <Switch>
                <Match when={source.state() === 'pending'}>
                  <div class="flex justify-center">
                    <LoadingSpinner />
                  </div>
                </Match>
                <Match when={source.state() === 'error'}>
                  <RunLoadFailed onRetry={source.retry} />
                </Match>
                <Match when={source.data()}>
                  {(run) => (
                    <RunContent
                      controller={props.controller}
                      run={run()}
                      time={formatRunTime}
                      statusRef={(element) => {
                        status = element;
                      }}
                    />
                  )}
                </Match>
              </Switch>
            </div>
          </MobileDrawer.ScrollBody>
        </MobileDrawer.Content>
      </MobileDrawer.Portal>
    </MobileDrawer>
  );
}
