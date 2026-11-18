import type {
  Gate,
  RunDetail,
  Step,
} from '../../../generated/factory/types.gen';
import { MacroError, unwrap } from '../../utils';
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
   * Answer the Gate the Run waits on, as its owner would, with one of the
   * answers it offers and an optional note for its later Steps. From an
   * agent session the factory takes only what a Standing Rule could
   * answer: an agent's question, a Tampered Run's approval and a merge
   * while the base branch has a Main Hold stay the owner's, and are
   * refused with code `reservedForAPerson`.
   *
   * @write
   */
  async answer(opts: { answer: string; note?: string }): Promise<void> {
    const waiting = await this.waitingGate();
    if (!waiting) {
      throw new MacroError(`Run ${this.id} waits on no gate`);
    }
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
