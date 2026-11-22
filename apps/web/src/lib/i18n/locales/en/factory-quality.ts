export default {
  loading: 'Loading CI health and review quality…',
  loadFailed: 'CI health and review quality couldn’t be loaded',
  repository: 'Repository',
  allRepositories: 'All repositories',
  runs: 'Runs',
  // CI health.
  ciHealth: 'CI health',
  ciHealthDescription:
    'How your runs’ pull requests fared in CI, and why their checks failed',
  redWaits: 'Red waits',
  redWaitsDefinition:
    'The share of waits on a pull request’s checks that ended red.',
  firstPushToGreen: 'First push to green',
  firstPushToGreenDefinition:
    'The median time from a pull request’s first wait on CI, once pushed, to its first green checks.',
  ciFixRounds: 'CI fix rounds',
  ciFixRoundsDefinition:
    'Rounds runs spent fixing failed checks. Each red wait sends the change to be fixed once.',
  reviewFixRounds: 'Review fix rounds',
  reviewFixRoundsDefinition:
    'Rounds runs spent fixing reviewers’ blockers, apart from CI.',
  causes: 'Failures by cause',
  causeOwn: 'The pull request’s',
  causeSetup: 'Setup',
  causeFlaky: 'Flaky',
  causeCancelled: 'Cancelled',
  causesNote:
    'Setup: the job failed before its own work ran. Flaky: the same test, or the same named step, failed on two or more pull requests. Cancelled: a newer push replaced the run, a matrix stopped it when another of its jobs failed, a concurrency group cancelled it, or a person did.',
  jobs: 'Jobs',
  job: 'Job',
  finished: 'Finished',
  redRate: 'Red rate',
  pullRequestsFailed: 'Pull requests failed',
  noJobs: 'No CI waits were recorded in this period.',
  recurringFailures: 'Failed on two or more pull requests',
  recurringFailuresDescription: 'Newest first',
  noRecurringFailures:
    'No test or named step failed on more than one pull request in this period.',
  failedStep: '{{job}} · {{step}}',
  recurringFailure: {
    one: '{{count}} failure on {{pullRequests}} pull requests, last {{date}}',
    other:
      '{{count}} failures on {{pullRequests}} pull requests, last {{date}}',
  },
  // Review quality.
  reviewQuality: 'Review quality',
  reviewQualityDescription:
    'How your runs’ review rounds went, and what their reviewers found',
  roundsPerRun: 'Rounds per run',
  roundsPerRunDefinition: 'Review rounds per run that had one or more.',
  cleanFirstRound: 'Clean after the first round',
  cleanFirstRoundDefinition:
    'The share of runs whose first review round found no blocker.',
  blockers: 'Blockers',
  blockersDefinition: 'Blockers the reviewers found.',
  repeatedBlockers: 'Repeated blockers',
  repeatedBlockersDefinition:
    'Blockers an earlier round of the same run found too: they were not fixed.',
  byReviewer: 'Blockers by reviewer',
  byAgent: 'Blockers by agent',
  reviews: 'Reviews',
  perReview: 'Per review',
  noReviews: 'No review rounds were recorded in this period.',
  recurringBlockers: 'Blockers found in two or more runs',
  recurringBlockersDescription:
    'One fault in a task template, brief or reviewer shows here as one row',
  noRecurringBlockers: 'No blocker was found in more than one run.',
  recurringBlocker: {
    one: '{{count}} run · {{reviewers}}',
    other: '{{count}} runs · {{reviewers}}',
  },
  // Cost and autonomy.
  delivery: 'Cost and autonomy',
  deliveryDescription:
    'What merging cost, and how often runs that ended on their own merged with no person stepping in and weren’t found flawed. Cancelled runs and infrastructure failures are left out.',
  autonomyRate: 'Autonomy rate',
  autonomyRateDefinition:
    'Runs that merged with no person stepping in and weren’t found flawed, of those that ended on their own: no gate a person answered, no step they steered, and no manual end. Runs still going, runs you cancelled and runs that failed for infrastructure are left out.',
  merged: 'Merged pull requests',
  mergedDefinition: 'Runs that merged their pull request.',
  costPerMerged: 'Cost per merged pull request',
  costPerMergedDefinition:
    'What every run’s agents cost, divided by the merged pull requests. In tokens when some model calls have no price.',
  tokensPerMerged: 'Tokens per merged pull request',
  steppedIn: 'A person stepped in',
  personAnswered: {
    one: '{{count}} run with a gate a person answered',
    other: '{{count}} runs with a gate a person answered',
  },
  steered: {
    one: '{{count}} run steered in the step terminal',
    other: '{{count}} runs steered in the step terminal',
  },
  manual: {
    one: '{{count}} run left to a person',
    other: '{{count}} runs left to a person',
  },
  unpriced: {
    one: '{{count}} model call has no price, so cost reads in tokens.',
    other: '{{count}} model calls have no price, so cost reads in tokens.',
  },
  unmeasured: {
    one: '{{count}} run has no recorded usage, so cost per merged pull request isn’t known.',
    other:
      '{{count}} runs have no recorded usage, so cost per merged pull request isn’t known.',
  },
};
