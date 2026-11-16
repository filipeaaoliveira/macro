import { t } from '@app/lib/i18n';
import { cn } from '@ui';
import { match } from 'ts-pattern';
import { flowWordLabel } from '../core/flow-words';
import type { RunOutcome, RunStatus } from '../core/run';

/**
 * A Run's state as a dot, in the colors Work gives an agent's sessions:
 * accent while it runs, warning while it waits on a person, failure when it
 * stopped short.
 */
export function RunStatusDot(props: { status: RunStatus }) {
  const tone = () =>
    match(props.status)
      .with({ kind: 'running' }, () => 'running' as const)
      .with({ kind: 'waiting' }, () => 'waiting' as const)
      .with({ kind: 'ended' }, (status) =>
        match(status.outcome)
          .with('success', () => 'done' as const)
          .with('failed', 'blocked', () => 'failed' as const)
          .with('manual', 'cancelled', () => 'idle' as const)
          .exhaustive()
      )
      .exhaustive();

  return (
    <span
      aria-hidden="true"
      class={cn(
        'size-1.5 shrink-0 rounded-full',
        tone() === 'running' && 'bg-accent motion-safe:animate-pulse',
        tone() === 'waiting' && 'bg-warning',
        tone() === 'done' && 'bg-success',
        tone() === 'failed' && 'bg-failure',
        tone() === 'idle' && 'bg-ink-disabled'
      )}
    />
  );
}

function runOutcomeLabel(outcome: RunOutcome): string {
  return match(outcome)
    .with('success', () => t('factoryRuns.outcomeSuccess'))
    .with('manual', () => t('factoryRuns.outcomeManual'))
    .with('blocked', () => t('factoryRuns.outcomeBlocked'))
    .with('failed', () => t('factoryRuns.outcomeFailed'))
    .with('cancelled', () => t('factoryRuns.outcomeCancelled'))
    .exhaustive();
}

/** One word for where the Run is. */
export function runStatusLabel(status: RunStatus): string {
  return match(status)
    .with({ kind: 'running' }, () => t('factoryRuns.running'))
    .with({ kind: 'waiting' }, () => t('factoryRuns.needsInput'))
    .with({ kind: 'ended' }, (ended) => runOutcomeLabel(ended.outcome))
    .exhaustive();
}

/** Where the Run is, naming its Step while it has not ended. */
export function runStatusDetail(status: RunStatus): string {
  return match(status)
    .with({ kind: 'running' }, ({ step }) =>
      step
        ? t('factoryRuns.runningStep', { step: flowWordLabel(step.entry) })
        : t('factoryRuns.starting')
    )
    .with({ kind: 'waiting' }, ({ step }) =>
      t('factoryRuns.waitingStep', { step: flowWordLabel(step.entry) })
    )
    .with({ kind: 'ended' }, (ended) => runOutcomeLabel(ended.outcome))
    .exhaustive();
}
