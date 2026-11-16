import { t } from '@app/lib/i18n';
import XIcon from '@icon/system/regular/x.svg';
import { ActionDialogShell, Button, Dialog } from '@ui';
import { createSignal, createUniqueId, type JSX, Show } from 'solid-js';
import { type Run, type RunStatus, runTitle } from '../core/run';
import { RunStatusDot, runStatusLabel } from './run-status';
import { RunTimeline } from './run-timeline';

/** The header of the Run dialog: its request's first line and the close. */
export function RunDetailsHeader(props: { title: string }) {
  return (
    <div class="flex shrink-0 items-center justify-between gap-3 border-b border-edge-divider px-5 py-4">
      <Dialog.Title class="min-w-0 truncate text-lg font-semibold">
        {props.title}
      </Dialog.Title>
      {/* Dialog's close button renders nothing on a phone sheet, which
          closes by its handle, backdrop or Escape. */}
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
 * A Run: where it is, what it was asked, its Steps, and the Gate it waits on
 * (`gate`) under the output it asks about, with the Gate's `answers` in the
 * footer so they stay in reach. `onCancel` is offered until the
 * Run ends, behind a confirmation in the dialog itself, as booking details
 * confirm a cancellation, so no second dialog stacks on a phone's sheet.
 */
export function RunDetails(props: {
  run: Run;
  status: RunStatus;
  gate?: JSX.Element;
  answers?: JSX.Element;
  /** When the Run started and ended, in full. */
  time: (value: string) => string;
  /** A Step's time, compact like the activity rows: `5m`. */
  stepTime: (value: string) => string;
  onCancel?: () => void;
  cancelling: boolean;
}) {
  const [confirming, setConfirming] = createSignal(false);
  const titleId = createUniqueId();
  const messageId = createUniqueId();
  let confirmation: HTMLDivElement | undefined;
  let keepButton: HTMLButtonElement | undefined;
  let cancelButton: HTMLButtonElement | undefined;
  const ended = () =>
    props.status.kind === 'ended' ? props.status.endedAt : undefined;

  const askToCancel = () => {
    setConfirming(true);
    queueMicrotask(() => {
      if (!keepButton?.isConnected) return;
      confirmation?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
      keepButton.focus();
    });
  };

  const keepRunning = () => {
    setConfirming(false);
    cancelButton?.focus();
  };

  return (
    <>
      <RunDetailsHeader title={runTitle(props.run)} />
      <ActionDialogShell.Body class="min-h-0 overflow-y-auto">
        <Show when={confirming() && props.onCancel}>
          {(cancel) => (
            <div
              ref={confirmation}
              class="flex flex-col gap-4 rounded-xl border border-edge-muted bg-surface-1 p-4"
            >
              <div class="text-sm">
                <p id={titleId} class="font-medium">
                  {t('factoryRuns.cancelTitle')}
                </p>
                <p id={messageId} class="mt-1 text-ink-muted">
                  {t('factoryRuns.cancelBody')}
                </p>
              </div>
              <div class="flex flex-wrap justify-end gap-2">
                <Button
                  ref={keepButton}
                  variant="ghost"
                  size="md"
                  aria-describedby={`${titleId} ${messageId}`}
                  disabled={props.cancelling}
                  onClick={keepRunning}
                >
                  {t('factoryRuns.keepRunning')}
                </Button>
                <Button
                  variant="danger"
                  size="md"
                  disabled={props.cancelling}
                  onClick={() => cancel()()}
                >
                  {t('factoryRuns.cancelRun')}
                </Button>
              </div>
            </div>
          )}
        </Show>
        <div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm">
          <span class="flex items-center gap-1.5 font-medium text-ink">
            <RunStatusDot status={props.status} />
            {runStatusLabel(props.status)}
          </span>
          <span class="text-ink-muted">
            {t('factoryRuns.startedAt', {
              time: props.time(props.run.createdAt),
            })}
          </span>
          <Show when={ended()}>
            {(endedAt) => (
              <span class="text-ink-muted">
                {t('factoryRuns.endedAt', { time: props.time(endedAt()) })}
              </span>
            )}
          </Show>
        </div>
        <section class="flex flex-col gap-1.5">
          <h3 class="text-xs font-medium text-ink-muted">
            {t('factoryRuns.request')}
          </h3>
          <p class="text-sm leading-5 whitespace-pre-wrap [overflow-wrap:anywhere]">
            {props.run.request}
          </p>
        </section>
        <section class="flex flex-col gap-1.5">
          <h3 class="text-xs font-medium text-ink-muted">
            {t('factoryRuns.steps')}
          </h3>
          <Show
            when={props.run.steps.length > 0}
            fallback={
              <p class="text-sm text-ink-placeholder">
                {t('factoryRuns.noSteps')}
              </p>
            }
          >
            <RunTimeline
              steps={props.run.steps}
              time={props.stepTime}
              outputOpen={(step) =>
                props.status.kind === 'waiting' &&
                props.status.step.entry === step.entry &&
                props.status.step.visit === step.visit
              }
            />
          </Show>
        </section>
        {props.gate}
      </ActionDialogShell.Body>
      {/* A Run waits on a Gate only before it ends, so the footer it
          needs for `answers` is there whenever they are. */}
      <Show when={props.onCancel}>
        <ActionDialogShell.Footer>
          <Button
            ref={cancelButton}
            type="button"
            variant="danger-ghost"
            class="mr-auto"
            disabled={props.cancelling || confirming()}
            onClick={askToCancel}
          >
            {t('factoryRuns.cancelRun')}
          </Button>
          {props.answers}
        </ActionDialogShell.Footer>
      </Show>
    </>
  );
}
