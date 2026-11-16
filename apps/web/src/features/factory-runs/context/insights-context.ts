import type { Accessor } from 'solid-js';
import { createContext, useContext } from 'solid-js';
import type { InsightsParams, RunInsights } from '../core/insights';
import type { RunsSource } from './runs-context';

/** What the insights view needs from the factory. */
export type InsightsContext = {
  /**
   * How the person's Runs went over a period; read again when it changes.
   * `updating` while another period's insights stand in for it.
   */
  insights: (
    params: Accessor<InsightsParams>
  ) => RunsSource<RunInsights> & { updating: Accessor<boolean> };
  /** The Flows a Run can be started from, to filter by. */
  flowNames: () => RunsSource<string[]>;
};

const Context = createContext<InsightsContext>();

export const InsightsProvider = Context.Provider;

export function useInsightsContext(): InsightsContext {
  const context = useContext(Context);
  if (!context) throw new Error('InsightsProvider is required');
  return context;
}
