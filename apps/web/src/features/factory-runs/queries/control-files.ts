import { throwOnErr } from '@core/util/result';
import { factoryClient } from '@service-factory/client';
import type { ControlFilesSettings } from '@service-factory/generated/schemas';
import { useMutation, useQuery, useQueryClient } from '@tanstack/solid-query';
import { factoryRunKeys } from './keys';

/** The Control File patterns the person's Runs are checked against. */
export function useControlFilesQuery() {
  return useQuery(() => ({
    queryKey: factoryRunKeys.controlFiles.queryKey,
    queryFn: ({ signal }) =>
      throwOnErr(() => factoryClient.getControlFiles(signal)),
  }));
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
