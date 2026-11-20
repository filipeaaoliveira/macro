import { throwOnErr } from '@core/util/result';
import { factoryClient } from '@service-factory/client';
import type { ControlFilesSettings } from '@service-factory/generated/schemas';
import {
  queryOptions,
  useMutation,
  useQuery,
  useQueryClient,
} from '@tanstack/solid-query';
import { factoryRunKeys } from './keys';

/**
 * The read of the Control File patterns, built at module scope so the cached
 * query keeps no view alive.
 */
function controlFilesOptions() {
  return queryOptions({
    queryKey: factoryRunKeys.controlFiles.queryKey,
    queryFn: ({ signal }) =>
      throwOnErr(() => factoryClient.getControlFiles(signal)),
  });
}

/** The Control File patterns the person's Runs are checked against. */
export function useControlFilesQuery() {
  return useQuery(() => controlFilesOptions());
}

/**
 * Sets the team's Control File patterns; `null` goes back to the default.
 * The settings it answers with replace the ones read.
 */
export function useSetControlFilesMutation() {
  const client = useQueryClient();
  return useMutation(() => ({
    mutationFn: (patterns: string[] | null) =>
      throwOnErr(() => factoryClient.setControlFiles(patterns)),
    onSuccess: (settings: ControlFilesSettings) => {
      client.setQueryData(factoryRunKeys.controlFiles.queryKey, settings);
    },
  }));
}
