import type { HandBackRequest } from '../../../generated/factory/types.gen';
import { unwrap } from '../../utils';
import type { MacroClient } from '../../utils/client';

/**
 * The software factory, as the agent doing one of a factory Run's Steps
 * sees it.
 *
 * @routing Use when a factory Run's prompt asks you to hand back its Step's result; not for any other work or session.
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
}
