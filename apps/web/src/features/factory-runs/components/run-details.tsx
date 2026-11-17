import { repositoryLabel } from '@app/features/agents-view/core/repository';
import { t } from '@app/lib/i18n';
import { Button } from '@ui';
import { createSignal, createUniqueId, Show } from 'solid-js';
import {
  type MainHold,
  type Run,
  type RunStatus,
  type RunStep,
  runRequestBody,
} from '../core/run';
import { RunStatusDot, runStatusLabel } from './run-status';
import { RunTimeline } from './run-timeline';

/**
 * Where the Run is, when it started and ended, the repository it works on,
 * and what its request says past the title. The status line takes focus
 * when the control a person used goes away (an answered Gate, a cancelled
 * Run).
 */
export function RunSummary(props: {
  run: Run;
  status: RunStatus;
  time: (value: string) => string;
  statusRef?: (element: HTMLElement) => void;
}) {
  const ended = () =>
    props.status.kind === 'ended' ? props.status.endedAt : undefined;

  return (
    <>
      <p
        ref={props.statusRef}
        data-run-status
        tabIndex={-1}
        class="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm outline-none"
      >
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
      </p>
      <Show when={props.run.repository}>
        {(repository) => (
          <section class="flex flex-col gap-1.5">
            <h3 class="text-xs font-medium text-ink-muted">
              {t('factoryRuns.repository')}
            </h3>
            <p class="text-sm leading-5 [overflow-wrap:anywhere]">
              {t('factoryRuns.repositoryFrom', {
                repository: repositoryLabel(repository().url),
                branch: repository().baseBranch,
              })}
            </p>
          </section>
        )}
      </Show>
      <Show when={runRequestBody(props.run)}>
        {(body) => (
          <section class="flex flex-col gap-1.5">
            <h3 class="text-xs font-medium text-ink-muted">
              {t('factoryRuns.request')}
            </h3>
            <p class="text-sm leading-5 whitespace-pre-wrap [overflow-wrap:anywhere]">
              {body()}
            </p>
          </section>
        )}
      </Show>
    </>
  );
}

/** The Run's Steps, the output a waiting Gate asks about open. */
export function RunSteps(props: {
  steps: RunStep[];
  /** The hold on the Run's base branch, shown on its latest Step. */
  mainHold?: MainHold;
  baseBranch?: string;
  status: RunStatus;
  time: (value: string) => string;
}) {
  return (
    <section class="flex flex-col gap-1.5">
      <h3 class="text-xs font-medium text-ink-muted">
        {t('factoryRuns.steps')}
      </h3>
      <Show
        when={props.steps.length > 0}
        fallback={
          <p class="text-sm text-ink-placeholder">{t('factoryRuns.noSteps')}</p>
        }
      >
        <RunTimeline
          steps={props.steps}
          mainHold={props.mainHold}
          baseBranch={props.baseBranch}
          time={props.time}
          outputOpen={(step) =>
            props.status.kind === 'waiting' &&
            props.status.step.seq === step.seq
          }
        />
      </Show>
    </section>
  );
}

/**
 * Cancel run, confirmed in place as booking details confirm a cancel, so no
 * second dialog stacks on a phone's sheet.
 */
export function CancelRun(props: {
  /** A cancel is on the wire, or sent and not yet read back. */
  cancelling: boolean;
  onCancel: () => void;
}) {
  const [confirming, setConfirming] = createSignal(false);
  const messageId = createUniqueId();
  let keepButton: HTMLButtonElement | undefined;
  let cancelButton: HTMLButtonElement | undefined;

  const askToCancel = () => {
    setConfirming(true);
    queueMicrotask(() => keepButton?.focus());
  };

  const keepRunning = () => {
    setConfirming(false);
    queueMicrotask(() => cancelButton?.focus());
  };

  return (
    <Show
      when={confirming()}
      fallback={
        <Button
          ref={cancelButton}
          type="button"
          variant="danger-ghost"
          disabled={props.cancelling}
          onClick={askToCancel}
        >
          {t('factoryRuns.cancelRun')}
        </Button>
      }
    >
      <div class="flex w-full flex-wrap items-center justify-end gap-2">
        <p id={messageId} class="mr-auto min-w-0 flex-1 basis-60 text-sm">
          <span class="font-medium">{t('factoryRuns.cancelTitle')}</span>{' '}
          <span class="text-ink-muted">{t('factoryRuns.cancelBody')}</span>
        </p>
        <Button
          ref={keepButton}
          type="button"
          variant="ghost"
          aria-describedby={messageId}
          disabled={props.cancelling}
          onClick={keepRunning}
        >
          {t('factoryRuns.keepRunning')}
        </Button>
        <Button
          type="button"
          variant="danger"
          loading={props.cancelling}
          onClick={() => props.onCancel()}
        >
          {t('factoryRuns.cancelRun')}
        </Button>
      </div>
    </Show>
  );
}
