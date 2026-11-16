import { t } from '@app/lib/i18n';
import { toast } from '@core/component/Toast/Toast';
import { fireAndForget } from '@core/util/fireAndForget';
import { Button } from '@ui';
import { Show } from 'solid-js';
import { GateCard } from '../../components/gate-card';
import { CancelRun, RunSteps, RunSummary } from '../../components/run-details';
import type { RunActionResult } from '../../context/runs-context';
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

function notifyCancel(result: RunActionResult | undefined) {
  if (!result || result.ok) return;
  if (result.reason === 'conflict') {
    toast.alert(t('factoryRuns.alreadyEnded'));
    return;
  }
  toast.failure(t('factoryRuns.cancelFailed'));
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
          />
          <RunSteps
            steps={props.run.steps}
            status={status()}
            time={props.controller.stepTime}
          />
        </>
      )}
    </Show>
  );
}

/** The Gate the Run waits on, with its answers: the view pins it in reach. */
export function RunGate(props: { controller: RunController }) {
  return (
    <Show when={props.controller.waiting()}>
      {(waiting) => (
        <GateCard
          question={waiting().gate.question}
          answers={waiting().gate.answers}
          note={props.controller.note()}
          onNoteChange={props.controller.setNote}
          locked={props.controller.gateLocked()}
          answering={props.controller.answering()}
          onAnswer={(value) =>
            fireAndForget(
              props.controller.answer(value).then(notifyAnswer),
              'factory-runs'
            )
          }
        />
      )}
    </Show>
  );
}

/** Cancel run, until the Run ends. */
export function RunCancel(props: { controller: RunController }) {
  return (
    <CancelRun
      cancelling={props.controller.cancelling()}
      onCancel={() =>
        fireAndForget(
          props.controller.cancel().then(notifyCancel),
          'factory-runs'
        )
      }
    />
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
