import { throwOnErr } from '@core/util/result';
import { factoryClient } from '@service-factory/client';
import {
  keepPreviousData,
  queryOptions,
  useQuery,
} from '@tanstack/solid-query';
import type { InsightsContext } from '../context/insights-context';
import type { InsightsParams } from '../core/insights';
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

/** The insights context on the factory service. */
export function createAppInsightsContext(): InsightsContext {
  return {
    insights: (params) => {
      const query = useQuery(() => insightsOptions(params()));
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
  };
}
