import type {
  FlowSummary,
  HandBackRequest,
} from '../../../generated/factory/types.gen';
import { unwrap } from '../../utils';
import type { MacroClient } from '../../utils/client';
import type { Task } from '../tasks/task';
import { FactoryRun } from './run';

/**
 * The software factory: Runs of its Flows, which agents work on and people
 * decide at Gates. Every call acts as the signed-in person, with their own
 * access, and reaches only their own Runs.
 *
 * @routing Use when starting, steering or answering software factory Runs, reading the Gates and questions waiting on the person, or when a factory Run's prompt asks you to hand back its Step's result; not for coding sessions or tasks outside the factory.
 *
 * @agent
 * Follow the Orchestrate Factory Runs skill. Start a Run only when the person
 * asks for the work. Answer a Gate only with the answer the person gave in
 * chat, passing the Gate you showed them (`run.answer(waiting, ...)`); a
 * 409 means it was answered or the Run moved on. Sending work back is
 * always yours to do; approving is only where their Orchestrator Definition
 * allows, and never a merge, an agent's question or a Tampered Run: the
 * factory refuses those (`reservedForAPerson`). A start refused with
 * `queueFull` waits until the person answers their Queue.
 *
 * @mcp-exclude the factory service runs only where code mode is handed its host
 */
export class FactoryNamespace {
  constructor(private readonly client: MacroClient) {}

  /** A handle to one of the caller's Runs by id. */
  byId(id: string): FactoryRun {
    return FactoryRun.byId(this.client, id);
  }

  /**
   * The built-in Flows a Run can be started of, by name.
   *
   * @readonly
   */
  async flows(): Promise<FlowSummary[]> {
    return unwrap(await this.client.factory.listFactoryFlows()).flows;
  }

  /**
   * Start a Run of the built-in Flow `flow` for `request`, owned by the
   * caller. A Flow that works on a repository, such as `code`, needs its
   * `repository` URL. Refused with code `queueFull` while more Gates and
   * questions wait on the caller than the Project's Queue limit.
   *
   * @example
   * const run = await macro.factory.start({
   *   flow: 'plan',
   *   request: 'Plan a theme toggle for the settings page.',
   * });
   *
   * @write
   */
  async start(opts: {
    flow: string;
    request: string;
    /** The task the Run works on. */
    task?: Task;
    /** The GitHub repository, as its `https://github.com/owner/name` URL. */
    repository?: string;
  }): Promise<FactoryRun> {
    const detail = unwrap(
      await this.client.factory.createFactoryRun({
        body: {
          flow: opts.flow,
          request: opts.request,
          taskId: opts.task?.id,
          repoUrl: opts.repository,
        },
      }),
    );
    return FactoryRun.byId(this.client, detail.run.id);
  }

  /**
   * The caller's Queue: their Runs that wait on them at a Gate or an
   * agent's question, newest first, at most 50.
   *
   * @readonly
   */
  async queue(): Promise<FactoryRun[]> {
    const { runs } = unwrap(await this.client.factory.listFactoryWaitingRuns());
    return runs.map((detail) => FactoryRun.byId(this.client, detail.run.id));
  }

  /**
   * The caller's Runs on `task`, newest first, at most 50.
   *
   * @readonly
   */
  async runsOf(task: Task): Promise<FactoryRun[]> {
    const { runs } = unwrap(
      await this.client.factory.listFactoryTaskRuns({
        path: { task_id: task.id },
      }),
    );
    return runs.map((detail) => FactoryRun.byId(this.client, detail.run.id));
  }

  /**
   * Hand back the result of the factory Step this agent session works on:
   * how it ended, one of the outcomes the Step's prompt offers, and what you
   * produced. Handing back again replaces the earlier result; the Step ends
   * with your turn, with your latest hand back.
   *
   * @example
   * await macro.factory.handBack({
   *   outcome: 'done',
   *   text: 'Plan: add a theme toggle to the settings page.',
   * });
   *
   * @mcp-exclude only an agent session a factory Run started can hand back
   */
  async handBack(result: HandBackRequest): Promise<void> {
    unwrap(await this.client.factory.handBackFactoryStep({ body: result }));
  }
}
