import type { Accessor } from 'solid-js';
import { createContext, useContext } from 'solid-js';
import type { EvaluationInsights } from '../core/evaluation';
import type { InsightsParams, RunInsights } from '../core/insights';
import type { QualityInsights, QualityParams } from '../core/quality';
import type { RunActionResult, RunsSource } from './runs-context';

/** What the insights view needs from the factory. */
export type InsightsContext = {
  /**
   * How the person's Runs went over a period; read again when it changes.
   * `updating` while another period's insights stand in for it.
   */
  insights: (
    params: Accessor<InsightsParams>
  ) => RunsSource<RunInsights> & { updating: Accessor<boolean> };
  /**
   * How the person's Runs over the period were judged by Scorers and split
   * by Experiments, and the Benchmark suites they replayed.
   */
  evaluation: (
    params: Accessor<InsightsParams>
  ) => RunsSource<EvaluationInsights> & { updating: Accessor<boolean> };
  /**
   * CI health, review quality, cost and autonomy of the person's Runs over
   * the period, of one repository or of all.
   */
  quality: (
    params: Accessor<QualityParams>
  ) => RunsSource<QualityInsights> & { updating: Accessor<boolean> };
  /** The Flows a Run can be started from, to filter by. */
  flowNames: () => RunsSource<string[]>;
  /**
   * Sends the failures the rejected self-improvement run `improvementId`
   * dismissed back to pending, then reads the evaluation again. `conflict`
   * means it holds no dismissed failure any more; `forbidden`, that the
   * person may no longer undo it (no longer an admin of the team, say).
   */
  undoDismissal: (improvementId: string) => Promise<UndoDismissalResult>;
  /**
   * Quarantines the CI job `job` of `repository` when `quarantine`, or
   * releases it, then reads quality again. A refusal carries the service's
   * message when it gave one: the person does not own or administer their
   * team, or may not push to the repository.
   */
  /**
   * Whether the person owns or administers their team, so may quarantine
   * and release CI jobs (the service also asks for push access).
   */
  isTeamAdmin: () => Accessor<boolean>;
  setCiQuarantine: (
    job: { repository: string; job: string },
    quarantine: boolean
  ) => Promise<CiQuarantineResult>;
};

/** How quarantining or releasing a CI job went. */
export type CiQuarantineResult = { ok: true } | { ok: false; message?: string };

/** How undoing a rejection's dismissal went. */
export type UndoDismissalResult =
  | RunActionResult
  | { ok: false; reason: 'forbidden' };

const Context = createContext<InsightsContext>();

export const InsightsProvider = Context.Provider;

export function useInsightsContext(): InsightsContext {
  const context = useContext(Context);
  if (!context) throw new Error('InsightsProvider is required');
  return context;
}
