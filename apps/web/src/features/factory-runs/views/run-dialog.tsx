import { t } from '@app/lib/i18n';
import { LoadingSpinner } from '@core/component/LoadingSpinner';
import XIcon from '@icon/system/regular/x.svg';
import { ActionDialogShell, Button, Dialog } from '@ui';
import { createSignal, Match, Show, Switch } from 'solid-js';
import { GateCard } from '../components/gate-card';
import { gateAsker, runTitle } from '../core/run';
import { createFocusRescue } from '../primitives/focus-rescue';
import type { RunController } from '../primitives/run-controller';
import {
  answerGate,
  RunCancel,
  RunContent,
  RunLoadFailed,
  replyToGate,
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

/** The Gate the Run waits on, pinned under the Steps. */
export function RunGate(props: { controller: RunController }) {
  return (
    <Show when={props.controller.waiting()}>
      {(waiting) => (
        <GateCard
          question={waiting().gate.question}
          asker={gateAsker(waiting().step, waiting().gate)}
          answers={waiting().gate.answers}
          gate={waiting().gate}
          time={formatRunTime}
          controlFiles={props.controller.controlFiles()}
          approveControlFiles={props.controller.approveControlFiles()}
          onApproveControlFilesChange={props.controller.setApproveControlFiles}
          held={props.controller.heldAnswers()}
          note={props.controller.note()}
          onNoteChange={props.controller.setNote}
          locked={props.controller.gateLocked()}
          answering={props.controller.answering()}
          onAnswer={(value) => answerGate(props.controller, value)}
          replying={props.controller.replying()}
          onReply={() => replyToGate(props.controller)}
        />
      )}
    </Show>
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
                <Show
                  when={props.controller.owned() && props.controller.waiting()}
                >
                  {/* Pinned; its question and note scroll inside past a few
                      lines, so the answers stay in reach. */}
                  <div class="shrink-0 border-t border-edge-divider px-5 pt-3">
                    <RunGate controller={props.controller} />
                  </div>
                </Show>
                <Show when={props.controller.owned() && !run().end}>
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
