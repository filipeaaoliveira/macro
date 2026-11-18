import { throwOnErr } from '@core/util/result';
import { factoryClient } from '@service-factory/client';
import type { DefinitionSettings } from '@service-factory/generated/schemas';
import { useMutation, useQuery, useQueryClient } from '@tanstack/solid-query';
import { factoryRunKeys } from './keys';

/** The Factory Definition the person's team names, read at its head. */
export function useDefinitionQuery() {
  return useQuery(() => ({
    queryKey: factoryRunKeys.definition.queryKey,
    queryFn: ({ signal }) =>
      throwOnErr(() => factoryClient.getDefinition(signal)),
  }));
}

/**
 * Names the team's Factory Definition; `null` names none. The settings it
 * answers with replace the ones read; the Control Files, Standing Rules and
 * Flows, which it decides, are read again.
 */
export function useSetDefinitionMutation() {
  const client = useQueryClient();
  return useMutation(() => ({
    mutationFn: (repoUrl: string | null) =>
      throwOnErr(() => factoryClient.setDefinition(repoUrl)),
    onSuccess: (settings: DefinitionSettings) => {
      client.setQueryData(factoryRunKeys.definition.queryKey, settings);
      const reads = [
        factoryRunKeys.controlFiles.queryKey,
        factoryRunKeys.standingRules.queryKey,
        factoryRunKeys.flows.queryKey,
      ].map((queryKey) => client.invalidateQueries({ queryKey }));
      return Promise.all(reads);
    },
  }));
}
