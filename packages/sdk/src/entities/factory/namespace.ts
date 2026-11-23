import type {
  FiringSkip,
  FlowSummary,
  HandBackRequest,
  RefusedTrial,
} from '../../../generated/factory/types.gen';
import { unwrap } from '../../utils';
import type { MacroClient } from '../../utils/client';
import type { Task } from '../tasks/task';
import { FactoryRun } from './run';

/** What fired a factory Automation, from a run of its routine. */
export interface AutomationFiring {
  /** The routine run that fires it: a retried run fires once. */
  run: { id: string };
  /** The task a Task event is about: the Run works on it. */
  task?: Task;
  /**
   * The GitHub delivery a repository event is, with the branch it is
   * about, so a factory Run's own pushes and pull request start nothing.
   */
  github?: { event: string; action?: string; branch?: string };
}

/** What firing a factory Automation came to. */
export interface AutomationFired {
  /** Whether it started a Run. */
  started: boolean;
  /**
   * Why it started none: the Automation is switched off, a Run already
   * runs on the task, the queue is full, or the factory refused the Run.
   */
  skipped?: FiringSkip;
  /** Whether this routine run fired it before, so it started nothing more. */
  repeated: boolean;
}

/** A Benchmark suite the caller started: one trial Run per frozen task. */
export interface BenchmarkSuite {
  /** The suite's id. */
  id: string;
  /** Each trial's Run, by the frozen task it replays. */
  runs: Record<string, FactoryRun>;
  /**
   * The trial whose Run the factory refused, which stopped the suite: the
   * trials before it go on.
   */
  refused?: RefusedTrial;
}

/**
 * The software factory: Runs of its Flows, which agents work on and people
 * decide at Gates. Every call acts as the signed-in person, with their own
 * access, and reaches only their own Runs. A factory Automation's routine
 * fires its Automation through it too.
 *
 * @routing Use when starting, steering or answering software factory Runs, reading the Gates and questions waiting on the person, when a factory Run's prompt asks you to hand back its Step's result, or in the code step of a factory Automation's routine; not for coding sessions or tasks outside the factory.
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
   * `repository` URL. Refused with code `queueFull` while as many of the
   * caller's Runs wait on them, or have yet to ask them anything, as the
   * Project's Queue limit.
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
   * The caller's Runs that hold a place in their Queue before asking them
   * anything: Runs of a Flow with a Gate that have not reached one yet,
   * newest first, at most 50. With the Runs in `queue()`, they are what a
   * Queue limit counts; cancelling one frees its place.
   *
   * @readonly
   */
  async starting(): Promise<FactoryRun[]> {
    const { starting } = unwrap(
      await this.client.factory.listFactoryWaitingRuns(),
    );
    return starting.map((detail) =>
      FactoryRun.byId(this.client, detail.run.id),
    );
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
   * Start a suite of the Benchmark `benchmark`: one trial Run, owned by the
   * caller, of each of its frozen tasks, on the configuration `agents`
   * gives (the Agent each Role takes in place of its first, by Role; a
   * Role left out takes the definition's). Each trial keeps a score and
   * the tokens its agents used once its Run ends. Refused (409) while a
   * suite of the same Benchmark still runs or the caller has too many
   * running trials, and (403 `notInStandingRole`) where the caller's
   * Standing Roles let no agent start Runs. A trial whose Run is refused
   * stops the suite: `refused` names it.
   *
   * @example
   * const suite = await macro.factory.startBenchmarkSuite('smoke', {
   *   agents: { implementer: 'fast' },
   * });
   *
   * @write
   */
  async startBenchmarkSuite(
    benchmark: string,
    opts: { agents?: Record<string, string> } = {},
  ): Promise<BenchmarkSuite> {
    const started = unwrap(
      await this.client.factory.startFactoryBenchmarkSuite({
        path: { benchmark },
        body: { agents: opts.agents },
      }),
    );
    return {
      id: started.id,
      runs: Object.fromEntries(
        Object.entries(started.runs).map(([task, id]) => [
          task,
          FactoryRun.byId(this.client, id),
        ]),
      ),
      refused: started.refused ?? undefined,
    };
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

  /**
   * Fire the factory Automation whose routine this run is: it starts a Run
   * of the Automation's Flow as the routine's owner, once per routine run,
   * unless the Automation is switched off or a Run already runs on the
   * task. Only the code step of an Automation's routine may call it.
   *
   * @example
   * await macro.factory.fireAutomation({ run });
   *
   * @mcp-exclude only a run of a factory Automation's routine fires it
   */
  async fireAutomation(firing: AutomationFiring): Promise<AutomationFired> {
    const fired = unwrap(
      await this.client.factory.fireFactoryAutomation({
        body: {
          firing: firing.run.id,
          taskId: firing.task?.id,
          github: firing.github,
        },
      }),
    );
    return {
      started: fired.runId !== undefined && fired.runId !== null,
      skipped: fired.skipped ?? undefined,
      repeated: fired.repeated,
    };
  }
}
