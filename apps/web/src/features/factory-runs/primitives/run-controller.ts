import { formatCompactRelativeTimestamp } from '@app/features/entity/utils/timestamp';
import { createMemo, createSignal, onCleanup } from 'solid-js';
import type {
  RetryRunResult,
  RunActionResult,
  RunsContext,
  RunsSource,
} from '../context/runs-context';
import {
  APPROVE_ANSWER,
  isQuestion,
  isRetriable,
  isTampered,
  QUESTION_ANSWER,
  type Run,
  type RunStep,
  retryOf,
  runStatus,
  type SteerAction,
} from '../core/run';

/** Whether `run` merges only once a person approves its Control Files. */
function tampered(run: Run): boolean {
  return run.controlFiles ? isTampered(run.controlFiles) : false;
}

/** How often Step times ("5m") move on. */
const CLOCK_MS = 30_000;

/** A Gate is its Step's: Steps are keyed by `seq`, as a reset restarts `visit`. */
function gateKey(step: Pick<RunStep, 'seq'>): number {
  return step.seq;
}

/**
 * One open Run's state and actions, shared by the desktop dialog and the
 * phone sheet: the Run as last read, the Gate it waits on, the note being
 * written, and answering, cancelling and retrying with their locks.
 * `openRun` shows another Run in its place: the Run it retries, or its
 * retry.
 */
export function createRunController(
  context: Pick<
    RunsContext,
    | 'run'
    | 'taskRuns'
    | 'answerGate'
    | 'replyToGate'
    | 'cancelRun'
    | 'retryRun'
    | 'steerStep'
    | 'viewerId'
  >,
  runId: () => string,
  openRun: (runId: string) => void = () => {}
) {
  const source: RunsSource<Run> = context.run(runId);
  const [note, setNote] = createSignal('');
  // Whether the person ticked their approval of a Tampered Run's files.
  const [approveControlFiles, setApproveControlFiles] = createSignal(false);
  // The Gate this person answered stays locked until the Run is read again
  // past it, so a second click can't send a second answer.
  const [answeredGate, setAnsweredGate] = createSignal<number>();
  const [answering, setAnswering] = createSignal<string>();
  // The person's words on their way to the Gate.
  const [replying, setReplying] = createSignal(false);
  const [cancelling, setCancelling] = createSignal(false);
  // A sent cancel keeps its controls locked until the Run reads as ended.
  const [cancelSent, setCancelSent] = createSignal(false);
  // This person's own answer or cancel went through, so the control they
  // used goes away once the Run is read past it.
  const [rescue, setRescue] = createSignal<'answer' | 'cancel'>();
  // The Step whose Step Terminal is open, by its place, and the note the
  // person writes its agent there.
  const [terminalSeq, setTerminalSeq] = createSignal<number>();
  const [terminalNote, setTerminalNote] = createSignal('');
  // The steer on the wire, which locks the Terminal's controls.
  const [steering, setSteering] = createSignal<SteerAction>();
  // A retry on the wire, and the Runs this person retried from here or the
  // factory refused to retry: the Retry button goes once one is sent,
  // before any list shows the retry.
  const [retrying, setRetrying] = createSignal(false);
  const [retried, setRetried] = createSignal<ReadonlySet<string>>(new Set());
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
    if (
      !run ||
      !gate ||
      answering() ||
      replying() ||
      cancelling() ||
      cancelSent()
    ) {
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
      // Only a Tampered Run's answer approves Control Files: the ones the
      // person saw, which the service checks are still the Run's.
      ...(tampered(run) && approveControlFiles()
        ? { approvedControlFiles: run.controlFiles?.tampered }
        : {}),
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

  // Words don't lock the Gate once sent: the factory may ask back on it,
  // and the person then answers or writes again.
  const reply = async (): Promise<RunActionResult | undefined> => {
    const run = source.data();
    const gate = waiting();
    const words = note().trim();
    if (
      !run ||
      !gate ||
      !words ||
      answering() ||
      replying() ||
      cancelling() ||
      cancelSent()
    ) {
      return undefined;
    }
    setReplying(true);
    const result = await context.replyToGate({
      runId: run.id,
      step: gate.step,
      reply: words,
    });
    setReplying(false);
    if (result.ok) setNote('');
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

  const terminalStep = createMemo(() => {
    const seq = terminalSeq();
    if (seq === undefined) return undefined;
    return source.data()?.steps.find((step) => step.seq === seq);
  });

  // A note goes with a pause's resume: while paused, the agent reads it
  // only once it goes on.
  const steer = async (
    action: SteerAction
  ): Promise<RunActionResult | undefined> => {
    const run = source.data();
    const step = terminalStep();
    if (!run || !step || step.end || steering()) return undefined;
    const note = terminalNote().trim();
    if (action === 'note' && !note) return undefined;
    setSteering(action);
    const result = await context.steerStep(
      action === 'note'
        ? { runId: run.id, step, action, note }
        : action === 'resume'
          ? { runId: run.id, step, action, note: note || undefined }
          : { runId: run.id, step, action }
    );
    setSteering(undefined);
    if (result.ok && (action === 'note' || action === 'resume')) {
      setTerminalNote('');
    }
    return result;
  };

  // Only its owner answers, cancels or steers a Run; a team member reads it.
  // The Run is the person's only once both its owner and they are known.
  const owned = () => {
    const owner = source.data()?.ownerId;
    const viewer = context.viewerId?.();
    return owner !== undefined && viewer !== undefined && owner === viewer;
  };

  // The owner's Runs of the task, read once the Run ended as a retry can
  // take over: its retry, if it has one, is among them.
  const taskRuns = context.taskRuns(() => {
    const run = source.data();
    if (!run?.taskId || !owned() || !isRetriable(run, false)) return '';
    return run.taskId;
  });

  const retriedBy = createMemo(() => {
    const run = source.data();
    const runs = taskRuns.data();
    return run ? retryOf(run, runs ?? []) : undefined;
  });

  const retriable = () => {
    const run = source.data();
    if (!run || !owned()) return false;
    return isRetriable(run, retriedBy() !== undefined || retried().has(run.id));
  };

  // Another Run shown in this one's place starts clean: no note, Gate
  // lock, focus rescue or Step Terminal of the Run before it.
  const open = (next: string) => {
    if (next === runId()) return;
    setNote('');
    setApproveControlFiles(false);
    setAnsweredGate(undefined);
    setRescue(undefined);
    setCancelSent(false);
    setTerminalSeq(undefined);
    setTerminalNote('');
    openRun(next);
  };

  const retry = async (): Promise<RetryRunResult | undefined> => {
    const run = source.data();
    if (!run || retrying() || !retriable()) return undefined;
    setRetrying(true);
    const result = await context.retryRun(run.id);
    setRetrying(false);
    // A retry that went through, and one the factory refused as retried
    // already or not retriable (a Run that kept no Flow to walk again), is
    // offered no more for this Run.
    if (result.ok || result.reason === 'conflict') {
      setRetried((ids) => new Set([...ids, run.id]));
    }
    if (result.ok) open(result.value.id);
    return result;
  };

  return {
    source,
    /** Whether the person owns the Run, and so acts on it. */
    owned,
    status,
    waiting,
    note,
    setNote,
    /** The Control Files the Run's branch changed unasked, while it waits. */
    controlFiles: () => (waiting() ? source.data()?.controlFiles : undefined),
    approveControlFiles,
    setApproveControlFiles,
    /**
     * The answers the person can't send yet: approving a Tampered Run before
     * they ticked their approval of its files, and answering an agent's
     * question before they wrote their answer.
     */
    heldAnswers: (): string[] => {
      const run = source.data();
      const gate = waiting()?.gate;
      if (gate && isQuestion(gate)) {
        return note().trim() ? [] : [QUESTION_ANSWER];
      }
      const held = gate && run && tampered(run);
      return held && !approveControlFiles() ? [APPROVE_ANSWER] : [];
    },
    /** The answer on the wire, if one is. */
    answering,
    /** Whether the person's words are on the wire. */
    replying,
    gateLocked: () => {
      const gate = waiting();
      if (!gate) return true;
      return (
        answering() !== undefined ||
        replying() ||
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
    reply,
    cancel,
    /** Whether the person can retry the Run: theirs, ended so, not retried. */
    retriable,
    /** Whether a retry is on the wire. */
    retrying,
    retry,
    /** The Run's retry, when it names one or its task's Runs list it. */
    retriedBy,
    /** Shows another Run in this one's place, with none of this one's state. */
    openRun: open,
    /** The Step whose Step Terminal is open, as last read. */
    terminalStep,
    openTerminal: (step: Pick<RunStep, 'seq'>) => {
      setTerminalNote('');
      setTerminalSeq(step.seq);
    },
    closeTerminal: () => setTerminalSeq(undefined),
    terminalNote,
    setTerminalNote,
    /** The steer on the wire, if one is. */
    steering,
    steer,
    /** Whose control a coming read takes away: this person's answer or cancel. */
    rescue,
    clearRescue: () => setRescue(undefined),
  };
}

export type RunController = ReturnType<typeof createRunController>;
