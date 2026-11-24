import { createAgentSession } from '@app/features/block-agent/context/create-agent-session';
import { useUserId } from '@core/context/user';
import { useAgentSessionSubscription } from '@queries/agent-session/subscription';
import { type Accessor, createMemo } from 'solid-js';
import { type ActivityLine, sessionActivity } from '../core/activity';

/**
 * The live tail of a running Step's agent session: what it did last, read
 * from the same session its Step Terminal shows, kept current while it runs.
 */
export function useStepActivity(
  sessionId: Accessor<string | undefined>,
  limit = 4
): Accessor<ActivityLine[]> {
  const userId = useUserId();
  const live = createAgentSession(sessionId, {
    userId,
    provisioning: () => false,
  });
  useAgentSessionSubscription(sessionId);
  return createMemo(() =>
    sessionId() ? sessionActivity(live.messages(), limit) : []
  );
}
