import type {
  FiringSkip,
  HandBackRequest,
} from '../../../generated/factory/types.gen';
import { unwrap } from '../../utils';
import type { MacroClient } from '../../utils/client';
import type { Task } from '../tasks/task';

/** What fired a factory Automation, from a run of its routine. */
export interface AutomationFiring {
  /** The routine run that fires it: a retried run fires once. */
  run: { id: string };
  /** The task a Task event is about: the Run works on it. */
  task?: Task;
  /** The GitHub delivery a repository event is. */
  github?: { event: string; action?: string };
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

/**
 * The software factory, as the agent doing one of a factory Run's Steps
 * sees it, and as the routine of a factory Automation fires it.
 *
 * @routing Use when a factory Run's prompt asks you to hand back its Step's result, or in the code step of a factory Automation's routine; not for any other work or session.
 */
export class FactoryNamespace {
  constructor(private readonly client: MacroClient) {}

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
