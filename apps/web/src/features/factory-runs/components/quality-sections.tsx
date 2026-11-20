import {
  InsightSection,
  Metric,
} from '@app/features/scheduling/components/insights-panel';
import { formatLocale, t } from '@app/lib/i18n';
import { For, type JSX, Show } from 'solid-js';
import {
  type BlockerCount,
  type BlockerResolutions,
  CI_CAUSES,
  type CiCause,
  type CiCauseCounts,
  type CiHealth,
  type DeliveryInsight,
  mergedCost,
  type QualityInsights,
  type ReviewQuality,
  repositoryName,
} from '../core/quality';
import {
  durationFigure,
  formatRunDate,
  formatShare,
  formatTokens,
} from './insight-format';
import { RunLinkList } from './run-link-list';

const CAUSE_LABELS: Record<CiCause, () => string> = {
  own: () => t('factoryQuality.causeOwn'),
  setup: () => t('factoryQuality.causeSetup'),
  flaky: () => t('factoryQuality.causeFlaky'),
  cancelled: () => t('factoryQuality.causeCancelled'),
};

export function ciCauseLabel(cause: CiCause): string {
  return CAUSE_LABELS[cause]();
}

/** Blockers per review, to one decimal: `1.5`, or `1,5` in German. */
function formatPerReview(perReview: number): string {
  return new Intl.NumberFormat(formatLocale(), {
    minimumFractionDigits: 1,
    maximumFractionDigits: 1,
  }).format(perReview);
}

/** An amount in US dollars: `$1.25`. */
function formatUsd(usd: number): string {
  return new Intl.NumberFormat(formatLocale(), {
    style: 'currency',
    currency: 'USD',
    maximumFractionDigits: usd < 10 ? 2 : 0,
  }).format(usd);
}

/** A share as a whole percentage for a Metric, or none. */
function percent(share: number | null | undefined): number | undefined {
  return share === null || share === undefined
    ? undefined
    : Math.round(share * 100);
}

function EmptyNote(props: { children: JSX.Element }) {
  return (
    <p class="px-4 py-8 text-center text-sm text-ink-muted">{props.children}</p>
  );
}

function MetricRow(props: { children: JSX.Element }) {
  return (
    <div class="grid grid-cols-1 divide-y divide-edge-muted @sm/quality:grid-cols-2 @3xl/quality:grid-cols-4 @3xl/quality:divide-x @3xl/quality:divide-y-0">
      {props.children}
    </div>
  );
}

function SubHeading(props: { title: string; description?: string }) {
  return (
    <div class="px-4 pt-4">
      <h3 class="text-xs font-medium text-ink-muted">{props.title}</h3>
      <Show when={props.description}>
        <p class="text-xs text-ink-muted">{props.description}</p>
      </Show>
    </div>
  );
}

function CauseCounts(props: { causes: CiCauseCounts }) {
  return (
    <dl
      aria-label={t('factoryQuality.causes')}
      class="grid grid-cols-2 gap-x-4 gap-y-3 px-4 py-3 @3xl/quality:grid-cols-4"
    >
      <For each={CI_CAUSES}>
        {(cause) => (
          <div class="flex min-w-0 flex-col gap-0.5">
            <dt class="text-xs text-ink-muted">{ciCauseLabel(cause)}</dt>
            <dd class="text-sm font-medium tabular-nums">
              {props.causes[cause]}
            </dd>
          </div>
        )}
      </For>
    </dl>
  );
}

function causesLine(causes: CiCauseCounts): string {
  return CI_CAUSES.filter((cause) => causes[cause] > 0)
    .map((cause) => `${ciCauseLabel(cause)} ${causes[cause]}`)
    .join(' · ');
}

function CiSection(props: {
  ci: CiHealth;
  multipleRepositories: boolean;
  onOpenRun: (runId: string) => void;
}) {
  const redShare = () =>
    props.ci.waits > 0 ? props.ci.redWaits / props.ci.waits : undefined;
  const toGreen = () =>
    durationFigure(props.ci.firstPushToGreen?.medianSeconds);
  const jobName = (repository: string | null | undefined, name: string) =>
    props.multipleRepositories && repository
      ? `${repositoryName(repository)} · ${name}`
      : name;

  return (
    <InsightSection
      title={t('factoryQuality.ciHealth')}
      description={t('factoryQuality.ciHealthDescription')}
    >
      <MetricRow>
        <Metric
          label={t('factoryQuality.redWaits')}
          definition={t('factoryQuality.redWaitsDefinition')}
          value={percent(redShare())}
          unit="%"
          lowerIsBetter
        />
        <Metric
          label={t('factoryQuality.firstPushToGreen')}
          definition={t('factoryQuality.firstPushToGreenDefinition')}
          value={toGreen().value}
          unit={toGreen().unit}
          lowerIsBetter
        />
        <Metric
          label={t('factoryQuality.ciFixRounds')}
          definition={t('factoryQuality.ciFixRoundsDefinition')}
          value={props.ci.ciFixRounds}
          lowerIsBetter
        />
        <Metric
          label={t('factoryQuality.reviewFixRounds')}
          definition={t('factoryQuality.reviewFixRoundsDefinition')}
          value={props.ci.reviewFixRounds}
          lowerIsBetter
        />
      </MetricRow>
      <SubHeading title={t('factoryQuality.causes')} />
      <CauseCounts causes={props.ci.causes} />
      <p class="px-4 text-xs text-ink-muted">
        {t('factoryQuality.causesNote')}
      </p>
      <SubHeading title={t('factoryQuality.jobs')} />
      <Show
        when={props.ci.jobs.length > 0}
        fallback={<EmptyNote>{t('factoryQuality.noJobs')}</EmptyNote>}
      >
        <div class="overflow-x-auto px-4 pb-2">
          <table class="w-full min-w-[32rem] text-sm">
            <thead class="text-xs text-ink-muted">
              <tr>
                <th scope="col" class="py-1 pr-3 text-left font-normal">
                  {t('factoryQuality.job')}
                </th>
                <th scope="col" class="py-1 pr-3 text-left font-normal">
                  {t('factoryQuality.redRate')}
                </th>
                <th scope="col" class="py-1 pr-3 text-left font-normal">
                  {t('factoryQuality.finished')}
                </th>
                <th scope="col" class="py-1 pr-3 text-left font-normal">
                  {t('factoryQuality.pullRequestsFailed')}
                </th>
                <th scope="col" class="py-1 text-left font-normal">
                  {t('factoryQuality.causes')}
                </th>
              </tr>
            </thead>
            <tbody>
              <For each={props.ci.jobs}>
                {(job) => (
                  <tr class="border-t border-edge-muted">
                    <th
                      scope="row"
                      class="max-w-64 truncate py-2 pr-3 text-left font-medium"
                    >
                      {jobName(job.repository, job.name)}
                    </th>
                    <td class="py-2 pr-3 tabular-nums">
                      {job.finished > 0 ? formatShare(job.redRate) : '–'}
                    </td>
                    <td class="py-2 pr-3 tabular-nums">{job.finished}</td>
                    <td class="py-2 pr-3 tabular-nums">
                      {job.pullRequestsFailed}
                    </td>
                    <td class="py-2 text-xs text-ink-muted">
                      {causesLine(job.causes) || '–'}
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </div>
      </Show>
      <SubHeading
        title={t('factoryQuality.recurringFailures')}
        description={t('factoryQuality.recurringFailuresDescription')}
      />
      <Show
        when={props.ci.recurring.length > 0}
        fallback={
          <EmptyNote>{t('factoryQuality.noRecurringFailures')}</EmptyNote>
        }
      >
        <ul
          aria-label={t('factoryQuality.recurringFailures')}
          class="flex flex-col divide-y divide-edge-muted"
        >
          <For each={props.ci.recurring}>
            {(failure) => (
              <li class="flex flex-col gap-2 px-4 py-3">
                <div class="flex flex-wrap items-baseline justify-between gap-x-3 gap-y-1">
                  <h4 class="min-w-0 truncate text-sm font-medium">
                    {failure.step
                      ? t('factoryQuality.failedStep', {
                          job: jobName(failure.repository, failure.job),
                          step: failure.step,
                        })
                      : jobName(failure.repository, failure.job)}
                  </h4>
                  <span class="shrink-0 rounded-full border border-edge-muted px-2 py-0.5 text-xs">
                    {ciCauseLabel(failure.cause)}
                  </span>
                </div>
                <Show when={failure.failure}>
                  {(test) => (
                    <p
                      class="min-w-0 truncate font-mono text-xs"
                      title={test()}
                    >
                      {test()}
                    </p>
                  )}
                </Show>
                <p class="text-xs text-ink-muted">
                  {t('factoryQuality.recurringFailure', {
                    count: failure.failures,
                    pullRequests: failure.pullRequests,
                    date: formatRunDate(failure.lastFailedAt),
                  })}
                </p>
                <RunLinkList
                  label={t('factoryQuality.runs')}
                  runs={failure.runs.map((run) => ({ run, detail: '' }))}
                  onOpenRun={props.onOpenRun}
                />
              </li>
            )}
          </For>
        </ul>
      </Show>
    </InsightSection>
  );
}

function BlockerTable(props: { title: string; counts: BlockerCount[] }) {
  return (
    <div class="min-w-0 px-4 pb-2">
      <table class="w-full text-sm" aria-label={props.title}>
        <caption class="pb-1 pt-4 text-left text-xs font-medium text-ink-muted">
          {props.title}
        </caption>
        <thead class="text-xs text-ink-muted">
          <tr>
            <th scope="col" class="py-1 pr-3 text-left font-normal">
              <span class="sr-only">{props.title}</span>
            </th>
            <th scope="col" class="py-1 pr-3 text-left font-normal">
              {t('factoryQuality.reviews')}
            </th>
            <th scope="col" class="py-1 pr-3 text-left font-normal">
              {t('factoryQuality.blockers')}
            </th>
            <th scope="col" class="py-1 text-left font-normal">
              {t('factoryQuality.perReview')}
            </th>
          </tr>
        </thead>
        <tbody>
          <For each={props.counts}>
            {(count) => (
              <tr class="border-t border-edge-muted">
                <th
                  scope="row"
                  class="max-w-48 truncate py-2 pr-3 text-left font-medium"
                >
                  {count.name}
                </th>
                <td class="py-2 pr-3 tabular-nums">{count.reviews}</td>
                <td class="py-2 pr-3 tabular-nums">{count.blockers}</td>
                <td class="py-2 tabular-nums">
                  {count.reviews > 0
                    ? formatPerReview(count.blockers / count.reviews)
                    : '–'}
                </td>
              </tr>
            )}
          </For>
        </tbody>
      </table>
    </div>
  );
}

/** What became of the Blockers given to a fix: fixed, or disputed and then upheld or dropped. */
export function BlockerResolutionMetrics(props: {
  resolutions: BlockerResolutions;
}) {
  return (
    <MetricRow>
      <Metric
        label={t('factoryQuality.fixedBlockers')}
        definition={t('factoryQuality.fixedBlockersDefinition')}
        value={props.resolutions.fixed}
      />
      <Metric
        label={t('factoryQuality.upheldDisputes')}
        definition={t('factoryQuality.upheldDisputesDefinition')}
        value={props.resolutions.disputedUpheld}
        lowerIsBetter
      />
      <Metric
        label={t('factoryQuality.droppedDisputes')}
        definition={t('factoryQuality.droppedDisputesDefinition')}
        value={props.resolutions.disputedDropped}
      />
      <Metric
        label={t('factoryQuality.dismissedFindings')}
        definition={t('factoryQuality.dismissedFindingsDefinition')}
        value={props.resolutions.dismissed}
        lowerIsBetter
      />
    </MetricRow>
  );
}

function ReviewSection(props: {
  review: ReviewQuality;
  onOpenRun: (runId: string) => void;
}) {
  const roundsPerRun = () => {
    const rounds = props.review.roundsPerRun;
    return rounds === null || rounds === undefined
      ? undefined
      : Math.round(rounds * 10) / 10;
  };

  return (
    <InsightSection
      title={t('factoryQuality.reviewQuality')}
      description={t('factoryQuality.reviewQualityDescription')}
    >
      <MetricRow>
        <Metric
          label={t('factoryQuality.roundsPerRun')}
          definition={t('factoryQuality.roundsPerRunDefinition')}
          value={roundsPerRun()}
          lowerIsBetter
        />
        <Metric
          label={t('factoryQuality.cleanFirstRound')}
          definition={t('factoryQuality.cleanFirstRoundDefinition')}
          value={percent(props.review.cleanFirstRate)}
          unit="%"
        />
        <Metric
          label={t('factoryQuality.blockers')}
          definition={t('factoryQuality.blockersDefinition')}
          value={props.review.blockers}
          lowerIsBetter
        />
        <Metric
          label={t('factoryQuality.repeatedBlockers')}
          definition={t('factoryQuality.repeatedBlockersDefinition')}
          value={props.review.repeatedBlockers}
          lowerIsBetter
        />
      </MetricRow>
      <BlockerResolutionMetrics resolutions={props.review.resolutions} />
      <Show
        when={props.review.reviewedRuns > 0}
        fallback={<EmptyNote>{t('factoryQuality.noReviews')}</EmptyNote>}
      >
        <div class="grid grid-cols-1 @3xl/quality:grid-cols-2">
          <BlockerTable
            title={t('factoryQuality.byReviewer')}
            counts={props.review.byReviewer}
          />
          <BlockerTable
            title={t('factoryQuality.byAgent')}
            counts={props.review.byAgent}
          />
        </div>
      </Show>
      <SubHeading
        title={t('factoryQuality.recurringBlockers')}
        description={t('factoryQuality.recurringBlockersDescription')}
      />
      <Show
        when={props.review.recurring.length > 0}
        fallback={
          <EmptyNote>{t('factoryQuality.noRecurringBlockers')}</EmptyNote>
        }
      >
        <ul
          aria-label={t('factoryQuality.recurringBlockers')}
          class="flex flex-col divide-y divide-edge-muted"
        >
          <For each={props.review.recurring}>
            {(blocker) => (
              <li class="flex flex-col gap-2 px-4 py-3">
                <p class="text-sm line-clamp-3">{blocker.text}</p>
                <p class="text-xs text-ink-muted">
                  {t('factoryQuality.recurringBlocker', {
                    count: blocker.runs,
                    reviewers: blocker.reviewers.join(', ') || '–',
                  })}
                </p>
                <RunLinkList
                  label={t('factoryQuality.runs')}
                  runs={blocker.runLinks.map((run) => ({ run, detail: '' }))}
                  onOpenRun={props.onOpenRun}
                />
              </li>
            )}
          </For>
        </ul>
      </Show>
    </InsightSection>
  );
}

function DeliverySection(props: { delivery: DeliveryInsight }) {
  const cost = () => mergedCost(props.delivery);

  return (
    <InsightSection
      title={t('factoryQuality.delivery')}
      description={t('factoryQuality.deliveryDescription')}
    >
      <MetricRow>
        <Metric
          label={t('factoryQuality.autonomyRate')}
          definition={t('factoryQuality.autonomyRateDefinition')}
          value={percent(props.delivery.autonomyRate)}
          unit="%"
        />
        <Metric
          label={t('factoryQuality.merged')}
          definition={t('factoryQuality.mergedDefinition')}
          value={props.delivery.merged}
        />
      </MetricRow>
      <dl class="grid grid-cols-1 gap-x-4 gap-y-3 px-4 py-3 @sm/quality:grid-cols-2">
        <div class="flex min-w-0 flex-col gap-0.5">
          <dt class="text-xs text-ink-muted">
            {cost()?.kind === 'tokens'
              ? t('factoryQuality.tokensPerMerged')
              : t('factoryQuality.costPerMerged')}
          </dt>
          <dd
            class="text-sm font-medium tabular-nums"
            data-testid="cost-per-merged"
          >
            {(() => {
              const read = cost();
              if (!read) return '–';
              return read.kind === 'money'
                ? formatUsd(read.usd)
                : formatTokens(read.tokens);
            })()}
          </dd>
        </div>
        <div class="flex min-w-0 flex-col gap-0.5">
          <dt class="text-xs text-ink-muted">
            {t('factoryQuality.steppedIn')}
          </dt>
          <dd class="flex flex-col text-sm">
            <span>
              {t('factoryQuality.personAnswered', {
                count: props.delivery.personAnswered,
              })}
            </span>
            <span>
              {t('factoryQuality.steered', { count: props.delivery.steered })}
            </span>
            <span>
              {t('factoryQuality.manual', { count: props.delivery.manual })}
            </span>
          </dd>
        </div>
      </dl>
      <Show when={props.delivery.unpricedCalls > 0}>
        <p class="px-4 pb-3 text-xs text-ink-muted">
          {t('factoryQuality.unpriced', {
            count: props.delivery.unpricedCalls,
          })}
        </p>
      </Show>
    </InsightSection>
  );
}

/**
 * CI health, review quality, and what merging cost and how often it took no
 * person, for the period's Runs of one repository or of all.
 */
export function QualitySections(props: {
  quality: QualityInsights;
  onOpenRun: (runId: string) => void;
}) {
  return (
    <div class="@container/quality flex min-w-0 flex-col gap-5">
      <CiSection
        ci={props.quality.ci}
        multipleRepositories={
          !props.quality.repository && props.quality.repositories.length > 1
        }
        onOpenRun={props.onOpenRun}
      />
      <ReviewSection
        review={props.quality.review}
        onOpenRun={props.onOpenRun}
      />
      <DeliverySection delivery={props.quality.delivery} />
    </div>
  );
}
