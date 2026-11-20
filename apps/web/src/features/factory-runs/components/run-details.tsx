import { repositoryLabel } from '@app/features/agents-view/core/repository';
import { t } from '@app/lib/i18n';
import { Button } from '@ui';
import { createSignal, createUniqueId, For, Show } from 'solid-js';
import {
  type AutomationTriggerKind,
  anyResolved,
  type BaseRequiredChecks,
  type MainHold,
  type ReviewRound,
  type Run,
  type RunStandingAct,
  type RunStatus,
  type RunStep,
  type RunTokenBudget,
  runRequestBody,
} from '../core/run';
import { formatTokens } from './insight-format';
import { RunStatusDot, runStatusLabel } from './run-status';
import { RunTimeline } from './run-timeline';

const AUTOMATION_TRIGGERS = {
  schedule: 'factoryRuns.automationTriggerSchedule',
  repository_event: 'factoryRuns.automationTriggerRepositoryEvent',
  webhook: 'factoryRuns.automationTriggerWebhook',
  task_event: 'factoryRuns.automationTriggerTaskEvent',
} as const satisfies Record<AutomationTriggerKind, string>;

const STANDING_ACTS = {
  start: 'factoryRuns.standingActStart',
  cancel: 'factoryRuns.standingActCancel',
  link: 'factoryRuns.standingActLink',
} as const;

/** An act shown with the Run: a Gate's answer is shown on its Step. */
type RunLevelAct = RunStandingAct & { act: keyof typeof STANDING_ACTS };

function isRunLevel(standing: RunStandingAct): standing is RunLevelAct {
  return standing.act in STANDING_ACTS;
}

function runLevelActs(run: Run): RunLevelAct[] {
  return (run.standingActs ?? []).filter(isRunLevel);
}

/**
 * What a Run's token budget says: what its agents used of it, or that the
 * budget stopped the Run, and why.
 */
export function tokenBudgetLine(budget: RunTokenBudget): string {
  const limit = formatTokens(budget.budget);
  const stop = budget.stop;
  if (stop?.reason === 'spent') {
    return t('factoryRuns.tokenBudgetSpent', {
      used: formatTokens(stop.used),
      budget: limit,
    });
  }
  if (stop?.reason === 'unread') {
    return t('factoryRuns.tokenBudgetUnread', { budget: limit });
  }
  if (budget.used === undefined) {
    return t('factoryRuns.tokenBudgetOf', { budget: limit });
  }
  if (budget.measured === false) {
    return t('factoryRuns.tokenBudgetUnmeasured', { budget: limit });
  }
  return t('factoryRuns.tokenBudgetUsed', {
    used: formatTokens(budget.used),
    budget: limit,
  });
}

/**
 * Where the Run is, when it started and ended, the repository it works on,
 * the Factory Definition commit it read, the Automation that started it,
 * its token budget and what its agents used of it, and what its request
 * says past the title. The status line takes focus
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
      <Show when={props.run.definition}>
        {(definition) => (
          <section class="flex flex-col gap-1.5">
            <h3 class="text-xs font-medium text-ink-muted">
              {t('factoryRuns.definition')}
            </h3>
            <p class="text-sm leading-5 [overflow-wrap:anywhere]">
              <a
                class="underline"
                href={`${definition().repoUrl}/tree/${definition().commit}`}
                target="_blank"
                rel="noopener noreferrer"
              >
                {t('factoryRuns.definitionAt', {
                  repository: repositoryLabel(definition().repoUrl),
                  commit: definition().commit.slice(0, 7),
                })}
              </a>
            </p>
          </section>
        )}
      </Show>
      <Show when={props.run.automation}>
        {(automation) => (
          <section class="flex flex-col gap-1.5">
            <h3 class="text-xs font-medium text-ink-muted">
              {t('factoryRuns.startedBy')}
            </h3>
            <p class="text-sm leading-5 [overflow-wrap:anywhere]">
              {t('factoryRuns.startedByAutomation', {
                name: automation().name,
                trigger: t(AUTOMATION_TRIGGERS[automation().trigger]),
              })}
            </p>
          </section>
        )}
      </Show>
      <Show when={props.run.tokenBudget}>
        {(budget) => (
          <section class="flex flex-col gap-1.5">
            <h3 class="text-xs font-medium text-ink-muted">
              {t('factoryRuns.tokenBudget')}
            </h3>
            <p
              data-token-budget
              class="text-sm leading-5 [overflow-wrap:anywhere]"
            >
              {tokenBudgetLine(budget())}
            </p>
          </section>
        )}
      </Show>
      <Show
        when={
          anyResolved(props.run.blockerResolutions) &&
          props.run.blockerResolutions
        }
      >
        {(resolutions) => (
          <section class="flex flex-col gap-1.5">
            <h3 class="text-xs font-medium text-ink-muted">
              {t('factoryRuns.blockerResolutions')}
            </h3>
            <p
              data-blocker-resolutions
              class="text-sm leading-5 [overflow-wrap:anywhere]"
            >
              {t('factoryRuns.blockerResolutionsLine', {
                fixed: resolutions().fixed,
                upheld: resolutions().disputedUpheld,
                dropped: resolutions().disputedDropped,
              })}
            </p>
          </section>
        )}
      </Show>
      <Show when={runLevelActs(props.run).length > 0}>
        <section class="flex flex-col gap-1.5">
          <h3 class="text-xs font-medium text-ink-muted">
            {t('factoryRuns.standingActs')}
          </h3>
          <ul class="flex flex-col gap-1 text-sm leading-5 [overflow-wrap:anywhere]">
            <For each={runLevelActs(props.run)}>
              {(standing) => (
                <li>
                  {t(STANDING_ACTS[standing.act], {
                    agent: standing.agent,
                    role: standing.standingRole,
                    time: props.time(standing.at),
                  })}
                </li>
              )}
            </For>
          </ul>
        </section>
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
  baseRequiredChecks?: BaseRequiredChecks;
  baseBranch?: string;
  reviewRounds?: ReviewRound[];
  status: RunStatus;
  time: (value: string) => string;
  /** Opens a running Step's Step Terminal. */
  onWatch?: (step: RunStep) => void;
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
          baseRequiredChecks={props.baseRequiredChecks}
          baseBranch={props.baseBranch}
          reviewRounds={props.reviewRounds}
          time={props.time}
          onWatch={props.onWatch}
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
