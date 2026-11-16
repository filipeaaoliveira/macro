import { formatCompactRelativeTimestamp } from '@app/features/entity/utils/timestamp';
import { createMemo, createSignal, onCleanup } from 'solid-js';
import type {
  RunActionResult,
  RunsContext,
  RunsSource,
} from '../context/runs-context';
import { type Run, type RunStep, runStatus } from '../core/run';

/** How often Step times ("5m") move on. */
const CLOCK_MS = 30_000;

function gateKey(step: Pick<RunStep, 'entry' | 'visit'>): string {
  return `${step.entry}:${step.visit}`;
}

/**
 * One open Run's state and actions, shared by the desktop dialog and the
 * phone sheet: the Run as last read, the Gate it waits on, the note being
 * written, and answering and cancelling with their locks.
 */
export function createRunController(
  context: Pick<RunsContext, 'run' | 'answerGate' | 'cancelRun'>,
  runId: () => string
) {
  const source: RunsSource<Run> = context.run(runId);
  const [note, setNote] = createSignal('');
  // The Gate this person answered stays locked until the Run is read again
  // past it, so a second click can't send a second answer.
  const [answeredGate, setAnsweredGate] = createSignal<string>();
  const [answering, setAnswering] = createSignal<string>();
  const [cancelling, setCancelling] = createSignal(false);
  // A sent cancel keeps its controls locked until the Run reads as ended.
  const [cancelSent, setCancelSent] = createSignal(false);
  const [now, setNow] = createSignal(Date.now());
  const clock = setInterval(() => setNow(Date.now()), CLOCK_MS);
  onCleanup(() => clearInterval(clock));

  const status = createMemo(() => {
    const run = source.data();
    return run ? runStatus(run) : undefined;
  });
  const waiting = createMemo(() => {
    const current = status();
    return current?.kind === 'waiting' ? current : undefined;
  });

  const answer = async (
    value: string
  ): Promise<RunActionResult | undefined> => {
    const run = source.data();
    const gate = waiting();
    if (!run || !gate || answering()) return undefined;
    setAnswering(value);
    const result = await context.answerGate({
      runId: run.id,
      step: gate.step,
      answer: value,
      note: note().trim() || undefined,
    });
    setAnswering(undefined);
    if (result.ok) {
      setAnsweredGate(gateKey(gate.step));
      setNote('');
    }
    return result;
  };

  const cancel = async (): Promise<RunActionResult | undefined> => {
    const run = source.data();
    if (!run || cancelling() || cancelSent()) return undefined;
    setCancelling(true);
    const result = await context.cancelRun(run.id);
    setCancelling(false);
    if (result.ok) setCancelSent(true);
    return result;
  };

  return {
    source,
    status,
    waiting,
    note,
    setNote,
    /** The answer on the wire, if one is. */
    answering,
    gateLocked: () => {
      const gate = waiting();
      if (!gate) return true;
      return answering() !== undefined || answeredGate() === gateKey(gate.step);
    },
    /** A cancel is on the wire, or was sent and the Run has not ended yet. */
    cancelling: () => cancelling() || cancelSent(),
    /** A Step's time, compact like the activity rows: `5m`. */
    stepTime: (value: string) => {
      now();
      return formatCompactRelativeTimestamp(value);
    },
    answer,
    cancel,
  };
}

export type RunController = ReturnType<typeof createRunController>;
