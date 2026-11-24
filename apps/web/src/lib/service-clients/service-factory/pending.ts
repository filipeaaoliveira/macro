/**
 * Factory API fields and endpoints the generated client does not have yet.
 *
 * HAND-WRITTEN until the factory service's OpenAPI document, its client and
 * the SDK are regenerated. Each type mirrors the service's own; once
 * `generated/` has it, delete it here and read the generated one.
 */
import type {
  DeliveryInsight,
  DurationStats,
  EntryInsight,
  InsightDay,
} from './generated/schemas';

/** `InsightDay`, with how long the day's Runs took. */
export type InsightDayWithDuration = InsightDay & {
  /**
   * How long its Runs took from creation to their end, cancelled Runs and
   * Runs still going left out; none when none ended so.
   */
  runDuration?: DurationStats | null;
};

/** `EntryInsight`, with what its agent Steps cost. */
export type EntryInsightWithCost = EntryInsight & {
  /**
   * What its agent Steps' model calls cost, in US dollars; none unless they
   * recorded usage and every call was priced.
   */
  costUsd?: number | null;
};

/** `DeliveryInsight`, with how long merging took. */
export type DeliveryInsightWithMergeTime = DeliveryInsight & {
  /**
   * How long the Runs that merged took from their creation to their merge;
   * none while none merged.
   */
  timeToMerge?: DurationStats | null;
};

/** The CI job to quarantine or release (`CiJobRequest`). */
export type CiJobRequest = {
  /** The repository's `https://github.com/owner/name` URL. */
  repository: string;
  /** The job's name. */
  job: string;
};

/** A CI job quarantined or released by hand (`CiQuarantineChange`). */
export type CiQuarantineChange = {
  /** The repository, by its URL in lower case. */
  repository: string;
  /** The job. */
  job: string;
  /** Whether anything changed. */
  changed: boolean;
};

/** `POST`: quarantine a CI job (`quarantine_factory_ci_job`). */
export const QUARANTINE_CI_JOB_URL = '/insights/ci-quarantines';

/** `POST`: release a CI job (`release_factory_ci_job`). */
export const RELEASE_CI_JOB_URL = '/insights/ci-quarantines/release';
