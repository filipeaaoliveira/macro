import {
  WorkDot,
  type WorkDotState,
} from '@app/features/agent-work/components/WorkList';
import { t } from '@app/lib/i18n';
import { match } from 'ts-pattern';
import { flowWordLabel, flowWordText } from '../core/flow-words';
import type { RunOutcome, RunStatus } from '../core/run';

/** A Run's state as Work's status dot. */
export function RunStatusDot(props: { status: RunStatus }) {
  const state = (): WorkDotState =>
    match(props.status)
      .with({ kind: 'running' }, () => 'running' as const)
      .with({ kind: 'waiting' }, () => 'needs_input' as const)
      .with({ kind: 'ended' }, (status) =>
        match(status.outcome)
          .with('success', () => 'done' as const)
          .with('failed', 'blocked', 'flawed', () => 'failed' as const)
          // The factory's plumbing failed, not the work: a neutral tone.
          .with('manual', 'cancelled', 'infrastructure', () => 'idle' as const)
          .exhaustive()
      )
      .exhaustive();

  return <WorkDot state={state()} />;
}

/** The word for how a Run ended. */
export function runOutcomeLabel(outcome: RunOutcome): string {
  return match(outcome)
    .with('success', () => t('factoryRuns.outcomeSuccess'))
    .with('manual', () => t('factoryRuns.outcomeManual'))
    .with('blocked', () => t('factoryRuns.outcomeBlocked'))
    .with('failed', () => t('factoryRuns.outcomeFailed'))
    .with('cancelled', () => t('factoryRuns.outcomeCancelled'))
    .with('infrastructure', () => t('factoryRuns.outcomeInfrastructure'))
    .with('flawed', () => t('factoryRuns.outcomeFlawed'))
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
        ? t('factoryRuns.runningStep', { step: flowWordText(step.entry) })
        : t('factoryRuns.starting')
    )
    .with({ kind: 'waiting' }, ({ step }) =>
      t('factoryRuns.waitingStep', { step: flowWordLabel(step.entry) })
    )
    .with({ kind: 'ended' }, (ended) => runOutcomeLabel(ended.outcome))
    .exhaustive();
}
