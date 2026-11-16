import { formatCompactRelativeTimestamp } from '@app/features/entity/utils/timestamp';
import { formatLocale, t } from '@app/lib/i18n';
import { StaticMarkdownContext } from '@core/component/LexicalMarkdown/component/core/StaticMarkdown';
import { LoadingSpinner } from '@core/component/LoadingSpinner';
import { toast } from '@core/component/Toast/Toast';
import { isMobile } from '@core/mobile/isMobile';
import { fireAndForget } from '@core/util/fireAndForget';
import { ActionDialogShell, Button, Dialog } from '@ui';
import {
  createMemo,
  createSignal,
  Match,
  onCleanup,
  Show,
  Switch,
} from 'solid-js';
import { GateAnswers, GateCard } from '../components/gate-card';
import { RunDetails, RunDetailsHeader } from '../components/run-details';
import { useRunsContext } from '../context/runs-context';
import { type Run, type RunStep, runStatus } from '../core/run';

function formatTime(value: string): string {
  return new Intl.DateTimeFormat(formatLocale(), {
    dateStyle: 'medium',
    timeStyle: 'short',
  }).format(new Date(value));
}

const CLOCK_MS = 30_000;

function gateKey(step: Pick<RunStep, 'entry' | 'visit'>): string {
  return `${step.entry}:${step.visit}`;
}

/** One Run in a dialog: its Steps, the Gate it waits on, and Cancel. */
export function RunDialog(props: { runId: string; onClose: () => void }) {
  const context = useRunsContext();
  const source = context.run(() => props.runId);
  const [open, setOpen] = createSignal(true);
  const [note, setNote] = createSignal('');
  // The Gate this person answered: it stays locked until the Run is read
  // again past it, so a second click can't send a second answer.
  const [answeredGate, setAnsweredGate] = createSignal<string>();
  const [answering, setAnswering] = createSignal(false);
  const [cancelling, setCancelling] = createSignal(false);
  // A sent cancel keeps its controls locked until the Run reads as ended.
  const [cancelSent, setCancelSent] = createSignal(false);

  // Step times read "5m"; they move on after the Run stops being polled.
  const [now, setNow] = createSignal(Date.now());
  const clock = setInterval(() => setNow(Date.now()), CLOCK_MS);
  onCleanup(() => clearInterval(clock));
  const stepTime = (value: string) => {
    now();
    return formatCompactRelativeTimestamp(value);
  };

  const close = () => {
    if (!open()) return;
    setOpen(false);
    props.onClose();
  };

  const answer = async (run: Run, step: RunStep, value: string) => {
    if (answering()) return;
    setAnswering(true);
    const result = await context.answerGate({
      runId: run.id,
      step,
      answer: value,
      note: note().trim() || undefined,
    });
    setAnswering(false);

    if (result.ok) {
      setAnsweredGate(gateKey(step));
      setNote('');
      return;
    }
    if (result.reason === 'conflict') {
      toast.alert(t('factoryRuns.answerConflict'), {
        subtext: t('factoryRuns.answerConflictDescription'),
      });
      return;
    }
    toast.failure(t('factoryRuns.answerFailed'));
  };

  const cancel = async (run: Run) => {
    setCancelling(true);
    const result = await context.cancelRun(run.id);
    setCancelling(false);
    if (result.ok) {
      setCancelSent(true);
      return;
    }
    if (result.reason === 'conflict') {
      toast.alert(t('factoryRuns.alreadyEnded'));
      return;
    }
    toast.failure(t('factoryRuns.cancelFailed'));
  };

  return (
    <Dialog
      open={open()}
      onOpenChange={(next) => !next && close()}
      position="center"
      animate
      class="w-full max-w-[min(42rem,calc(100vw-16px))] overflow-hidden"
    >
      {/* On a phone the drawer's own maxHeight sizes the sheet. */}
      <ActionDialogShell
        class={isMobile() ? undefined : 'max-h-[calc(100dvh-16px)]'}
      >
        <Switch>
          <Match when={source.state() === 'pending'}>
            <RunDetailsHeader title={t('factoryRuns.runDetails')} />
            <ActionDialogShell.Body class="flex justify-center">
              <LoadingSpinner />
            </ActionDialogShell.Body>
          </Match>
          <Match when={source.state() === 'error'}>
            <RunDetailsHeader title={t('factoryRuns.runDetails')} />
            <ActionDialogShell.Body class="flex items-center gap-2">
              <p class="text-sm text-ink-muted">
                {t('factoryRuns.runLoadFailed')}
              </p>
              <Button variant="ghost" size="xs" onClick={source.retry}>
                {t('common.retry')}
              </Button>
            </ActionDialogShell.Body>
          </Match>
          <Match when={source.data()}>
            {(run) => {
              const status = createMemo(() => runStatus(run()));
              const waiting = createMemo(() => {
                const current = status();
                if (current.kind !== 'waiting') return undefined;
                return current;
              });
              const gateLocked = () => {
                const gate = waiting();
                if (!gate) return true;
                return answering() || answeredGate() === gateKey(gate.step);
              };

              return (
                <StaticMarkdownContext>
                  <RunDetails
                    run={run()}
                    status={status()}
                    time={formatTime}
                    stepTime={stepTime}
                    cancelling={cancelling() || cancelSent()}
                    onCancel={
                      status().kind === 'ended'
                        ? undefined
                        : () => fireAndForget(cancel(run()), 'factory-runs')
                    }
                    gate={
                      <Show when={waiting()}>
                        <GateCard
                          question={waiting()?.gate.question ?? ''}
                          note={note()}
                          onNoteChange={setNote}
                          disabled={gateLocked()}
                        />
                      </Show>
                    }
                    answers={
                      <Show when={waiting()}>
                        {(gate) => (
                          <GateAnswers
                            answers={gate().gate.answers}
                            disabled={gateLocked()}
                            onAnswer={(value) =>
                              fireAndForget(
                                answer(run(), gate().step, value),
                                'factory-runs'
                              )
                            }
                          />
                        )}
                      </Show>
                    }
                  />
                </StaticMarkdownContext>
              );
            }}
          </Match>
        </Switch>
      </ActionDialogShell>
    </Dialog>
  );
}
