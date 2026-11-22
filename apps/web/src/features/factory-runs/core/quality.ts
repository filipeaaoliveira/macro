import type {
  CiCause,
  DeliveryInsight,
} from '@service-factory/generated/schemas';
import type { InsightsParams } from './insights';

export type {
  BlockerCount,
  CiCause,
  CiCauseCounts,
  CiHealth,
  CiJobHealth,
  DeliveryInsight,
  QualityInsights,
  RecurringBlocker,
  RecurringCiFailure,
  ReviewQuality,
} from '@service-factory/generated/schemas';

/** What CI health and review quality are read for. */
export type QualityParams = InsightsParams & {
  /** A repository's URL, or none for every repository. */
  repository?: string;
};

/** The causes, in the order a breakdown lists them. */
export const CI_CAUSES: readonly CiCause[] = [
  'own',
  'setup',
  'flaky',
  'cancelled',
];

/** `owner/name` of a `https://github.com/owner/name` URL. */
export function repositoryName(url: string): string {
  return url.replace(/^https:\/\/github\.com\//, '');
}

/**
 * Cost per merged pull request as it reads: money when every model call was
 * priced, tokens otherwise, or none before a pull request merged or while
 * some Runs' usage is unknown (`unmeasuredRuns`).
 */
export function mergedCost(
  delivery: DeliveryInsight
):
  | { kind: 'money'; usd: number }
  | { kind: 'tokens'; tokens: number }
  | undefined {
  if (
    delivery.costPerMergedUsd !== undefined &&
    delivery.costPerMergedUsd !== null
  ) {
    return { kind: 'money', usd: delivery.costPerMergedUsd };
  }
  if (
    delivery.tokensPerMerged !== undefined &&
    delivery.tokensPerMerged !== null
  ) {
    return { kind: 'tokens', tokens: delivery.tokensPerMerged };
  }
  return undefined;
}
