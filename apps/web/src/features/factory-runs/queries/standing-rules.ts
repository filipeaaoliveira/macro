import { throwOnErr } from '@core/util/result';
import { factoryClient } from '@service-factory/client';
import type { StandingRulesSettings } from '@service-factory/generated/schemas';
import { useMutation, useQuery, useQueryClient } from '@tanstack/solid-query';
import type { OrchestratorDefinition } from '../core/standing-rules';
import { factoryRunKeys } from './keys';

/** The Standing Rules the person's Runs start with. */
export function useStandingRulesQuery() {
  return useQuery(() => ({
    queryKey: factoryRunKeys.standingRules.queryKey,
    queryFn: ({ signal }) =>
      throwOnErr(() => factoryClient.getStandingRules(signal)),
  }));
}

/**
 * Sets the team's Standing Rules; `null` goes back to the default. The
 * settings it answers with replace the ones read.
 */
export function useSetStandingRulesMutation() {
  const client = useQueryClient();
  return useMutation(() => ({
    mutationFn: (definition: OrchestratorDefinition | null) =>
      throwOnErr(() => factoryClient.setStandingRules(definition)),
    onSuccess: (settings: StandingRulesSettings) => {
      client.setQueryData(factoryRunKeys.standingRules.queryKey, settings);
    },
  }));
}
