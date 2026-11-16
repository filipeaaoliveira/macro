import { t } from '@app/lib/i18n';
import { LoadingSpinner } from '@core/component/LoadingSpinner';
import XIcon from '@icon/system/regular/x.svg';
import { ActionDialogShell, Button, Dialog } from '@ui';
import { createSignal, Match, Show, Switch } from 'solid-js';
import { runTitle } from '../core/run';
import { createFocusRescue } from '../primitives/focus-rescue';
import type { RunController } from '../primitives/run-controller';
import {
  RunCancel,
  RunContent,
  RunGate,
  RunLoadFailed,
} from './shared/run-parts';
import { formatRunTime } from './shared/run-time';

function RunDialogHeader(props: { title: string }) {
  return (
    <div class="flex shrink-0 items-center justify-between gap-3 border-b border-edge-divider px-5 py-4">
      <Dialog.Title class="min-w-0 truncate text-lg font-semibold">
        {props.title}
      </Dialog.Title>
      <Dialog.CloseButton
        as={Button}
        aria-label={t('factoryRuns.closeRun')}
        tooltip={{ content: t('factoryRuns.closeRun'), disabled: true }}
        variant="ghost"
        size="icon-sm"
      >
        <XIcon aria-hidden="true" class="size-4" />
      </Dialog.CloseButton>
    </div>
  );
}

/**
 * The Run on desktop: its Steps scroll, and the Gate it waits on and Cancel
 * run stay pinned under them, so the question is beside its answers.
 */
export function RunDialog(props: {
  controller: RunController;
  onClose: () => void;
}) {
  const [open, setOpen] = createSignal(true);
  let content: HTMLDivElement | undefined;
  let status: HTMLElement | undefined;
  const source = props.controller.source;
  createFocusRescue(
    props.controller,
    () => content,
    () => status
  );

  const close = () => {
    if (!open()) return;
    setOpen(false);
    props.onClose();
  };

  return (
    <Dialog
      open={open()}
      onOpenChange={(next) => !next && close()}
      contentRef={(element) => {
        content = element;
      }}
      position="center"
      animate
      class="w-full max-w-[min(42rem,calc(100vw-16px))] overflow-hidden"
    >
      <ActionDialogShell class="max-h-[calc(100dvh-16px)]">
        <Switch>
          <Match when={source.state() === 'pending'}>
            <RunDialogHeader title={t('factoryRuns.runDetails')} />
            <ActionDialogShell.Body class="flex justify-center">
              <LoadingSpinner />
            </ActionDialogShell.Body>
          </Match>
          <Match when={source.state() === 'error'}>
            <RunDialogHeader title={t('factoryRuns.runDetails')} />
            <ActionDialogShell.Body>
              <RunLoadFailed onRetry={source.retry} />
            </ActionDialogShell.Body>
          </Match>
          <Match when={source.data()}>
            {(run) => (
              <>
                <RunDialogHeader title={runTitle(run())} />
                <ActionDialogShell.Body class="min-h-0 overflow-y-auto">
                  <RunContent
                    controller={props.controller}
                    run={run()}
                    time={formatRunTime}
                    statusRef={(element) => {
                      status = element;
                    }}
                  />
                </ActionDialogShell.Body>
                <Show when={props.controller.waiting()}>
                  <div class="shrink-0 border-t border-edge-divider px-5 pt-3">
                    <RunGate controller={props.controller} />
                  </div>
                </Show>
                <Show when={!run().end}>
                  <ActionDialogShell.Footer>
                    <RunCancel controller={props.controller} />
                  </ActionDialogShell.Footer>
                </Show>
              </>
            )}
          </Match>
        </Switch>
      </ActionDialogShell>
    </Dialog>
  );
}
