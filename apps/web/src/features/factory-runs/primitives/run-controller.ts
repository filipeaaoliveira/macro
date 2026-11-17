import { formatCompactRelativeTimestamp } from '@app/features/entity/utils/timestamp';
import { createMemo, createSignal, onCleanup } from 'solid-js';
import type {
  RunActionResult,
  RunsContext,
  RunsSource,
} from '../context/runs-context';
import { APPROVE_ANSWER, type Run, type RunStep, runStatus } from '../core/run';

/** How often Step times ("5m") move on. */
const CLOCK_MS = 30_000;

/** A Gate is its Step's: Steps are keyed by `seq`, as a reset restarts `visit`. */
function gateKey(step: Pick<RunStep, 'seq'>): number {
  return step.seq;
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
  // Whether the person ticked their approval of a Tampered Run's files.
  const [approveControlFiles, setApproveControlFiles] = createSignal(false);
  // The Gate this person answered stays locked until the Run is read again
  // past it, so a second click can't send a second answer.
  const [answeredGate, setAnsweredGate] = createSignal<number>();
  const [answering, setAnswering] = createSignal<string>();
  const [cancelling, setCancelling] = createSignal(false);
  // A sent cancel keeps its controls locked until the Run reads as ended.
  const [cancelSent, setCancelSent] = createSignal(false);
  // This person's own answer or cancel went through, so the control they
  // used goes away once the Run is read past it.
  const [rescue, setRescue] = createSignal<'answer' | 'cancel'>();
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
    // A cancel on its way or sent ends the Run: the Gate takes no answer.
    if (!run || !gate || answering() || cancelling() || cancelSent()) {
      return undefined;
    }
    setAnswering(value);
    // Set before the answer is sent: the Run can be read past it first.
    setRescue('answer');
    const result = await context.answerGate({
      runId: run.id,
      step: gate.step,
      answer: value,
      note: note().trim() || undefined,
      // Only a Tampered Run's answer approves Control Files.
      ...(run.tampering ? { approveControlFiles: approveControlFiles() } : {}),
    });
    setAnswering(undefined);
    if (result.ok) {
      setAnsweredGate(gateKey(gate.step));
      setNote('');
      setApproveControlFiles(false);
    } else if (rescue() === 'answer') {
      // A refused answer (409) arrives with the Run read again. When that
      // read took the Gate away, focus has already moved; when it still
      // shows the same Gate, nothing of this answer goes away, so a later
      // poll must not move focus either. A failed request drops the rescue
      // too, and a cancel sent meanwhile keeps its own.
      const still = waiting();
      const sameGate =
        still !== undefined && gateKey(still.step) === gateKey(gate.step);
      if (result.reason === 'failed' || sameGate) setRescue(undefined);
    }
    return result;
  };

  const cancel = async (): Promise<RunActionResult | undefined> => {
    const run = source.data();
    if (!run || cancelling() || cancelSent()) return undefined;
    setCancelling(true);
    setRescue('cancel');
    const result = await context.cancelRun(run.id);
    setCancelling(false);
    if (result.ok) setCancelSent(true);
    else if (rescue() === 'cancel') setRescue(undefined);
    return result;
  };

  return {
    source,
    status,
    waiting,
    note,
    setNote,
    /** The Control Files the Run's branch changed unasked, while it waits. */
    tampering: () => (waiting() ? source.data()?.tampering : undefined),
    approveControlFiles,
    setApproveControlFiles,
    /**
     * The answers the person can't send yet: approving a Tampered Run before
     * they ticked their approval of its files.
     */
    heldAnswers: (): string[] => {
      const held = waiting() && source.data()?.tampering;
      return held && !approveControlFiles() ? [APPROVE_ANSWER] : [];
    },
    /** The answer on the wire, if one is. */
    answering,
    gateLocked: () => {
      const gate = waiting();
      if (!gate) return true;
      return (
        answering() !== undefined ||
        answeredGate() === gateKey(gate.step) ||
        cancelling() ||
        cancelSent()
      );
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
    /** Whose control a coming read takes away: this person's answer or cancel. */
    rescue,
    clearRescue: () => setRescue(undefined),
  };
}

export type RunController = ReturnType<typeof createRunController>;
