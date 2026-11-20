import { useFeatureFlag } from '@app/lib/analytics/posthog';
import { t } from '@app/lib/i18n';
import { enableFactoryRuns } from '@core/constant/featureFlags';
import { factoryClient } from '@service-factory/client';
import { queryOptions, useQuery } from '@tanstack/solid-query';
import { type ParentProps, Show } from 'solid-js';
import { factoryRunKeys } from '../queries/keys';

/** The app's link to a Run. */
export function runLink(runId: string): string {
  return `/app/factory/runs/${encodeURIComponent(runId)}`;
}

/**
 * The factory Step of the person's that works in the agent session
 * `sessionId`, if one does: none while it is read, and for any other
 * session.
 */
function useSessionStep(sessionId: () => string | undefined) {
  return useQuery(() => sessionStepOptions(sessionId()));
}

/**
 * The read of the factory Step working in the session `id`, built at
 * module scope so the cached query keeps no view alive.
 */
function sessionStepOptions(id: string | undefined) {
  return queryOptions({
    queryKey: factoryRunKeys.sessionStep(id ?? '').queryKey,
    queryFn: async ({ signal }) => {
      const result = await factoryClient.getSessionStep(id ?? '', signal);
      // Not a Step's session, or not the person's: the composer stays.
      return result.isOk() ? result.value : null;
    },
    enabled: Boolean(id),
    staleTime: Number.POSITIVE_INFINITY,
  });
}

/**
 * The session's composer, unless a factory Step of the person's works in
 * the session: then a note sends them to the Step's Run, where they steer
 * its agent and the Step keeps what they did.
 */
export function StepSessionComposerGate(
  props: ParentProps<{ sessionId: string | undefined }>
) {
  // Without the factory, no session is a Step's: nothing is read.
  const flag = useFeatureFlag(enableFactoryRuns);
  return (
    <Show when={flag().enabled} fallback={props.children}>
      <StepSessionGate sessionId={props.sessionId}>
        {props.children}
      </StepSessionGate>
    </Show>
  );
}

function StepSessionGate(
  props: ParentProps<{ sessionId: string | undefined }>
) {
  const step = useSessionStep(() => props.sessionId);
  return (
    <Show when={step.data} fallback={props.children}>
      {(found) => (
        <p
          class="rounded-md border border-edge-divider px-3 py-2 text-sm text-ink-muted"
          data-factory-step-session
        >
          {t('factoryRuns.stepSessionNotice')}{' '}
          <a
            class="font-medium text-accent underline"
            href={runLink(found().runId)}
          >
            {t('factoryRuns.stepSessionOpenRun')}
          </a>
        </p>
      )}
    </Show>
  );
}
