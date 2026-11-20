import { fireAndForget } from '@core/util/fireAndForget';
import { throwOnErr } from '@core/util/result';
import { factoryClient } from '@service-factory/client';
import {
  keepPreviousData,
  type QueryClient,
  queryOptions,
  useQuery,
} from '@tanstack/solid-query';
import type { InsightsContext } from '../context/insights-context';
import type { InsightsParams } from '../core/insights';
import type { QualityParams } from '../core/quality';
import { factoryRunKeys } from './keys';
import { flowsOptions, toSource } from './runs';

/** Insights move on as Runs go; a period read a minute ago is read again. */
const INSIGHTS_STALE_MS = 60_000;

export function insightsOptions(params: InsightsParams) {
  return queryOptions({
    queryKey: factoryRunKeys.insights(params).queryKey,
    queryFn: ({ signal }) =>
      throwOnErr(() =>
        factoryClient.getInsights(
          {
            from: params.from,
            to: params.to,
            timeZone: params.timeZone,
            flow: params.flow,
          },
          signal
        )
      ),
    staleTime: INSIGHTS_STALE_MS,
    // A new period or Flow keeps the last one on screen while it loads.
    placeholderData: keepPreviousData,
  });
}

export function evaluationOptions(params: InsightsParams) {
  return queryOptions({
    queryKey: factoryRunKeys.evaluation(params).queryKey,
    queryFn: ({ signal }) =>
      throwOnErr(() =>
        factoryClient.getEvaluation(
          {
            from: params.from,
            to: params.to,
            timeZone: params.timeZone,
            flow: params.flow,
          },
          signal
        )
      ),
    staleTime: INSIGHTS_STALE_MS,
    placeholderData: keepPreviousData,
  });
}

export function qualityOptions(params: QualityParams) {
  return queryOptions({
    queryKey: factoryRunKeys.quality(params).queryKey,
    queryFn: ({ signal }) =>
      throwOnErr(() =>
        factoryClient.getQuality(
          {
            from: params.from,
            to: params.to,
            timeZone: params.timeZone,
            flow: params.flow,
            repository: params.repository,
          },
          signal
        )
      ),
    staleTime: INSIGHTS_STALE_MS,
    placeholderData: keepPreviousData,
  });
}

/** The insights context on the factory service. */
export function createAppInsightsContext(client: QueryClient): InsightsContext {
  return {
    insights: (params) => {
      const query = useQuery(() => insightsOptions(params()));
      return {
        ...toSource(query),
        updating: () => query.isPlaceholderData,
      };
    },

    evaluation: (params) => {
      const query = useQuery(() => evaluationOptions(params()));
      return {
        ...toSource(query),
        updating: () => query.isPlaceholderData,
      };
    },

    quality: (params) => {
      const query = useQuery(() => qualityOptions(params()));
      return {
        ...toSource(query),
        updating: () => query.isPlaceholderData,
      };
    },

    flowNames: () => {
      const flows = toSource(useQuery(() => flowsOptions()));
      return {
        ...flows,
        data: () => flows.data()?.map((flow) => flow.name),
      };
    },

    async undoDismissal(improvementId) {
      const result =
        await factoryClient.undoImprovementDismissal(improvementId);
      // Either way, the evaluation read shows where the run stands now: its
      // failures pending again, or why there was nothing to undo.
      const reads = client.invalidateQueries({
        queryKey: factoryRunKeys.evaluation._def,
      });
      if (result.isErr()) {
        const coded = (code: string) =>
          result.error.some((error) => error.code === code);
        fireAndForget(reads, 'factory-runs');
        if (coded('CONFLICT')) return { ok: false, reason: 'conflict' };
        if (coded('FORBIDDEN')) return { ok: false, reason: 'forbidden' };
        return { ok: false, reason: 'failed' };
      }
      await reads.catch(() => undefined);
      return { ok: true, value: undefined };
    },
  };
}
