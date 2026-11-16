import { createEffect, createRoot } from 'solid-js';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { InsightsParams } from '../core/insights';
import { createInsightsState } from './insights-state';

afterEach(() => {
  vi.useRealTimers();
});

describe('createInsightsState', () => {
  it('moves to the new day when the page stays open past midnight', () => {
    vi.useFakeTimers();
    // Two minutes before midnight in Lisbon (UTC in November).
    vi.setSystemTime(new Date('2026-11-16T23:58:00Z'));
    const read: InsightsParams[] = [];

    const dispose = createRoot((dispose) => {
      const state = createInsightsState('Europe/Lisbon', () => new Date());
      createEffect(() => read.push(state.params()));
      return dispose;
    });
    // The tick at 23:59 leaves the day alone; the one at midnight moves it.
    vi.advanceTimersByTime(60_000);
    vi.advanceTimersByTime(60_000);
    dispose();

    expect(read.map((params) => params.to)).toEqual([
      '2026-11-16',
      '2026-11-17',
    ]);
  });
});
