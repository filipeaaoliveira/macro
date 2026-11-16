import { createMemo, createSignal } from 'solid-js';
import {
  type InsightPeriod,
  type InsightsParams,
  insightsParams,
} from '../core/insights';

/** The period and Flow the insights view shows, and the Run opened from it. */
export function createInsightsState(timeZone: string, now: () => Date) {
  const [period, setPeriod] = createSignal<InsightPeriod>(30);
  const [flow, setFlow] = createSignal<string>();
  const [openRunId, setOpenRunId] = createSignal<string>();
  const params = createMemo<InsightsParams>(() =>
    insightsParams(period(), timeZone, flow(), now())
  );

  return {
    period,
    setPeriod,
    flow,
    setFlow,
    params,
    openRunId,
    setOpenRunId,
    clearFilters: () => {
      setPeriod(30);
      setFlow(undefined);
    },
  };
}

export type InsightsState = ReturnType<typeof createInsightsState>;
