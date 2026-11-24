import { throwOnErr } from '@core/util/result';
import { factoryClient } from '@service-factory/client';
import type { AutomationRequest } from '@service-factory/generated/schemas';
import {
  queryOptions,
  useMutation,
  useQuery,
  useQueryClient,
} from '@tanstack/solid-query';
import { factoryRunKeys } from './keys';

/**
 * The read of the team's Automations, built at module scope so the cached
 * query keeps no view alive.
 */
export function automationsOptions() {
  return queryOptions({
    queryKey: factoryRunKeys.automations.queryKey,
    queryFn: ({ signal }) =>
      throwOnErr(() => factoryClient.listAutomations(signal)),
  });
}

/** The person's team's Automations. */
export function useAutomationsQuery() {
  return useQuery(() => automationsOptions());
}

/**
 * The changes a person makes to their team's Automations; each reads the
 * list again once it lands.
 */
export function useAutomationMutations() {
  const client = useQueryClient();
  const refresh = () =>
    client.invalidateQueries({
      queryKey: factoryRunKeys.automations.queryKey,
    });
  return {
    create: useMutation(() => ({
      mutationFn: (request: AutomationRequest) =>
        throwOnErr(() => factoryClient.createAutomation(request)),
      onSuccess: refresh,
    })),
    setEnabled: useMutation(() => ({
      mutationFn: (change: { id: string; enabled: boolean }) =>
        throwOnErr(() =>
          factoryClient.setAutomationEnabled(change.id, change.enabled)
        ),
      onSuccess: refresh,
    })),
    remove: useMutation(() => ({
      mutationFn: (id: string) =>
        throwOnErr(() => factoryClient.deleteAutomation(id)),
      onSuccess: refresh,
    })),
  };
}
