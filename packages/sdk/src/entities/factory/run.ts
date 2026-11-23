import type {
  Gate,
  RunDetail,
  Step,
} from '../../../generated/factory/types.gen';
import { unwrap } from '../../utils';
import type { MacroClient } from '../../utils/client';
import { Task } from '../tasks/task';

/** A Gate a factory Run waits on, with the Step that asks it. */
export interface WaitingGate {
  /** The Step whose end asks it. */
  step: Step;
  /** The Gate: its question, the answers it offers and who asks it. */
  gate: Gate;
}

/**
 * A Run of a software factory Flow: the Steps it took, the Gates they
 * asked and how it ended. Every Run is its owner's: it reads, answers and
 * cancels only as them.
 *
 * @mcp-exclude the factory service runs only where code mode is handed its host
 */
export class FactoryRun {
  private constructor(
    private readonly client: MacroClient,
    public readonly id: string,
  ) {}

  /** A handle to one of the caller's Runs by id. */
  static byId(client: MacroClient, id: string): FactoryRun {
    return new FactoryRun(client, id);
  }

  /**
   * The Run as it is now: its request, its Steps and their Gates, its
   * review rounds and how it ended, if it did.
   *
   * @readonly
   */
  async read(): Promise<RunDetail> {
    return unwrap(
      await this.client.factory.getFactoryRun({ path: { id: this.id } }),
    );
  }

  /**
   * The Gate the Run waits on, if it waits on one: a Flow's question, such
   * as approving the plan, or an agent's question to its owner.
   *
   * @readonly
   */
  async waitingGate(): Promise<WaitingGate | undefined> {
    const detail = await this.read();
    if (detail.run.end) return undefined;
    for (const step of [...detail.steps].reverse()) {
      if (step.gate && !step.gate.answer) {
        return { step, gate: step.gate };
      }
    }
    return undefined;
  }

  /**
   * Answer the Gate you read with {@link waitingGate} and decided on, as
   * the Run's owner told you, with one of the answers it offers and an
   * optional note for its later Steps. If the Run no longer waits on that
   * Gate (someone answered it, or the Run moved on), the factory refuses
   * the answer (409): read the Run again and ask the person. From an agent
   * session the factory takes sending work back (`revise`, `stop`), and
   * the approving answer (`approve`, `fix`) only where the Run's Standing
   * Rules let the Orchestrator give it; it never takes an agent's
   * question, a merge's approval or a Tampered Run's, which it refuses
   * with code `reservedForAPerson`.
   *
   * @example
   * const waiting = await run.waitingGate();
   * if (waiting) await run.answer(waiting, { answer: 'revise', note: 'Add tests first.' });
   *
   * @write
   */
  async answer(
    waiting: WaitingGate,
    opts: { answer: string; note?: string },
  ): Promise<void> {
    unwrap(
      await this.client.factory.answerFactoryGate({
        path: { id: this.id, seq: waiting.step.seq },
        body: { answer: opts.answer, note: opts.note },
      }),
    );
  }

  /**
   * Cancel the Run: its running Step or Gate stops. Cancelling a
   * cancelled Run again changes nothing.
   *
   * @destructive
   */
  async cancel(): Promise<void> {
    unwrap(
      await this.client.factory.cancelFactoryRun({ path: { id: this.id } }),
    );
  }

  /**
   * Retry the Run, if it ended `failed`, `infrastructure` or `blocked`: a
   * new Run of the same task, Flow and Factory Definition commit, on the
   * same branch, that keeps the Steps taken before the entry the Run
   * stopped at, with their outputs (its plan, its branch), and starts
   * there. The new Run's `retry.retriedFrom` names this one. A Run is
   * retried once: a second retry, a Run that is still going or ended
   * otherwise, and a retry past the person's Queue limit (code
   * `queueFull`) are refused (409). Retry the newest retry instead.
   *
   * @example
   * const retry = await run.retry();
   * const { run: started } = await retry.read();
   *
   * @write
   */
  async retry(): Promise<FactoryRun> {
    const detail = unwrap(
      await this.client.factory.retryFactoryRun({ path: { id: this.id } }),
    );
    return FactoryRun.byId(this.client, detail.run.id);
  }

  /**
   * Link the Run to the task it works on, replacing any task it had.
   *
   * @write
   */
  async linkTask(task: Task): Promise<void> {
    unwrap(
      await this.client.factory.linkFactoryRunTask({
        path: { id: this.id },
        body: { taskId: task.id },
      }),
    );
  }

  /**
   * The task the Run works on, if it has one.
   *
   * @readonly
   */
  async task(): Promise<Task | undefined> {
    const { run } = await this.read();
    return run.taskId ? Task.byId(this.client, run.taskId) : undefined;
  }

  /** The Run's id, so serialized handles say which Run they are. */
  toJSON(): { id: string } {
    return { id: this.id };
  }
}
