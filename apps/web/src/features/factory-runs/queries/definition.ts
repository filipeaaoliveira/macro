import { throwOnErr } from '@core/util/result';
import { factoryClient } from '@service-factory/client';
import type {
  DefinitionSettings,
  ProposeDefinitionChangeRequest,
} from '@service-factory/generated/schemas';
import {
  queryOptions,
  useMutation,
  useQuery,
  useQueryClient,
} from '@tanstack/solid-query';
import { factoryRunKeys } from './keys';

/** How the team's Factory Definition is read. */
export function definitionOptions() {
  return queryOptions({
    queryKey: factoryRunKeys.definition.queryKey,
    queryFn: ({ signal }) =>
      throwOnErr(() => factoryClient.getDefinition(signal)),
  });
}

/** The Factory Definition the person's team names, read at its head. */
export function useDefinitionQuery() {
  return useQuery(() => definitionOptions());
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

/**
 * Proposes a change to the team's Factory Definition as a pull request; the
 * Definition is read again, so its open pull requests list the new one.
 */
export function useProposeDefinitionChangeMutation() {
  const client = useQueryClient();
  return useMutation(() => ({
    mutationFn: (request: ProposeDefinitionChangeRequest) =>
      throwOnErr(() => factoryClient.proposeDefinitionChange(request)),
    onSuccess: () =>
      client.invalidateQueries({
        queryKey: factoryRunKeys.definition.queryKey,
      }),
  }));
}
