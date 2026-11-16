import { createMemo, createSignal, onCleanup, untrack } from 'solid-js';
import {
  type InsightPeriod,
  type InsightsParams,
  insightsParams,
} from '../core/insights';

/** How often the view checks whether a new day began. */
const CLOCK_MS = 60_000;

const sameParams = (a: InsightsParams, b: InsightsParams) =>
  a.from === b.from &&
  a.to === b.to &&
  a.timeZone === b.timeZone &&
  a.flow === b.flow;

/**
 * The period and Flow the insights view shows, and the Run opened from it.
 * The period ends today: a page left open past midnight moves to the new
 * day within a minute.
 */
export function createInsightsState(timeZone: string, now: () => Date) {
  const [period, setPeriod] = createSignal<InsightPeriod>(30);
  const [flow, setFlow] = createSignal<string>();
  const [openRunId, setOpenRunId] = createSignal<string>();
  const [clock, setClock] = createSignal(now());
  const timer = setInterval(() => setClock(now()), CLOCK_MS);
  onCleanup(() => clearInterval(timer));
  // A tick that leaves the days alone reads nothing again.
  const read = () => insightsParams(period(), timeZone, flow(), clock());
  const params = createMemo<InsightsParams>(read, untrack(read), {
    equals: sameParams,
  });

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
