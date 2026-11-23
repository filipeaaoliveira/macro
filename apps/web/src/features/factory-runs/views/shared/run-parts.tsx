import { t } from '@app/lib/i18n';
import { toast } from '@core/component/Toast/Toast';
import { fireAndForget } from '@core/util/fireAndForget';
import { Button } from '@ui';
import { Show } from 'solid-js';
import { CancelRun, RunSteps, RunSummary } from '../../components/run-details';
import { RunFlawNote } from '../../components/run-flaw';
import { RunScores } from '../../components/run-scores';
import type {
  RetryRunResult,
  RunActionResult,
} from '../../context/runs-context';
import type { Run } from '../../core/run';
import type { RunController } from '../../primitives/run-controller';

function notifyAnswer(result: RunActionResult | undefined) {
  if (!result || result.ok) return;
  if (result.reason === 'conflict') {
    toast.alert(t('factoryRuns.answerConflict'), {
      subtext: t('factoryRuns.answerConflictDescription'),
    });
    return;
  }
  toast.failure(t('factoryRuns.answerFailed'));
}

function notifyReply(result: RunActionResult | undefined) {
  if (!result || result.ok) return;
  if (result.reason === 'conflict') {
    toast.alert(t('factoryRuns.answerConflict'), {
      subtext: t('factoryRuns.answerConflictDescription'),
    });
    return;
  }
  toast.failure(t('factoryRuns.replyFailed'));
}

function notifyCancel(result: RunActionResult | undefined) {
  if (!result || result.ok) return;
  if (result.reason === 'conflict') {
    toast.alert(t('factoryRuns.alreadyEnded'));
    return;
  }
  toast.failure(t('factoryRuns.cancelFailed'));
}

function notifyRetry(result: RetryRunResult | undefined) {
  if (!result || result.ok) return;
  if (result.reason === 'queueFull') {
    toast.alert(t('factoryRuns.retryQueueFull'));
    return;
  }
  if (result.reason === 'conflict') {
    toast.alert(t('factoryRuns.retryConflict'), {
      subtext: t('factoryRuns.retryConflictDescription'),
    });
    return;
  }
  toast.failure(t('factoryRuns.retryFailed'));
}

/** What scrolls: where the Run is, its request, and its Steps. */
export function RunContent(props: {
  controller: RunController;
  run: Run;
  time: (value: string) => string;
  statusRef: (element: HTMLElement) => void;
}) {
  return (
    <Show when={props.controller.status()}>
      {(status) => (
        <>
          <RunSummary
            run={props.run}
            status={status()}
            time={props.time}
            statusRef={props.statusRef}
            retriedBy={props.controller.retriedBy()?.id}
            onOpenRun={props.controller.openRun}
          />
          <Show when={props.run.end?.flaw}>
            {(flaw) => <RunFlawNote flaw={flaw()} time={props.time} />}
          </Show>
          {/* Only the Run's owner reads and labels its Scorers' judgments. */}
          <Show when={props.controller.owned() && props.run.end}>
            {(end) => (
              <RunScores runId={props.run.id} endedAt={end().endedAt} />
            )}
          </Show>
          <RunSteps
            steps={props.run.steps}
            mainHold={props.run.mainHold}
            baseRequiredChecks={props.run.baseRequiredChecks}
            baseBranch={props.run.repository?.baseBranch}
            reviewRounds={props.run.reviewRounds}
            status={status()}
            time={props.controller.stepTime}
            onWatch={
              props.controller.owned()
                ? props.controller.openTerminal
                : undefined
            }
          />
        </>
      )}
    </Show>
  );
}

/**
 * Sends an answer and says what went wrong, if anything. Resolves whether
 * the answer went through, or was already on its way.
 */
export async function answerGate(
  controller: RunController,
  value: string
): Promise<boolean> {
  const result = await controller.answer(value);
  notifyAnswer(result);
  return result === undefined || result.ok;
}

/**
 * Sends the person's words to the Gate and says what went wrong, if
 * anything. Resolves whether they went through.
 */
export async function replyToGate(controller: RunController): Promise<boolean> {
  const result = await controller.reply();
  notifyReply(result);
  return result === undefined || result.ok;
}

async function cancelRun(controller: RunController) {
  notifyCancel(await controller.cancel());
}

/** Cancel run, until the Run ends. */
export function RunCancel(props: { controller: RunController }) {
  return (
    <CancelRun
      cancelling={props.controller.cancelling()}
      onCancel={() =>
        fireAndForget(cancelRun(props.controller), 'factory-runs')
      }
    />
  );
}

async function retryRun(controller: RunController) {
  notifyRetry(await controller.retry());
}

/**
 * Retry run, for its owner once it ended `failed`, `infrastructure` or
 * `blocked` and no retry of it is known: a new Run starts where it stopped,
 * and opens in its place.
 */
export function RunRetry(props: { controller: RunController }) {
  return (
    <Button
      type="button"
      variant="secondary"
      loading={props.controller.retrying()}
      onClick={() => fireAndForget(retryRun(props.controller), 'factory-runs')}
    >
      {t('factoryRuns.retryRun')}
    </Button>
  );
}

/** While the Run can't be read: retry. */
export function RunLoadFailed(props: { onRetry: () => void }) {
  return (
    <div class="flex items-center gap-2">
      <p class="text-sm text-ink-muted">{t('factoryRuns.runLoadFailed')}</p>
      <Button variant="ghost" size="xs" onClick={() => props.onRetry()}>
        {t('common.retry')}
      </Button>
    </div>
  );
}
