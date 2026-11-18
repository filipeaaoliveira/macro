export default {
  loading: 'Loading scorers, experiments and benchmarks…',
  loadFailed: 'Scorers, experiments and benchmarks couldn’t be loaded',
  // Scorers.
  scorers: 'Scorers',
  scorersDescription:
    'A small model judges a sample of your finished runs, one dimension per scorer',
  noScorers: 'No scorers are set up for this workspace.',
  sampled: '{{share}} of runs',
  sampledFlows: '{{share}} of {{flows}} runs',
  retired: 'No longer set up',
  judged: 'Judged',
  meanScore: 'Mean score',
  passed: 'Passed',
  // Self-improvement.
  selfImprovement: 'Self-improvement',
  selfImprovementDescription:
    'Failed judgments and flawed runs wait here until a run against your factory definition addresses them. A person merges its pull request.',
  pendingFailures: 'Pending failures',
  startsAt: 'Starts a run at',
  startsAtValue: {
    one: '{{count}} failure or {{days}} days',
    other: '{{count}} failures or {{days}} days',
  },
  oldestPending: 'Oldest pending',
  improvementOff: 'Turned off in the factory definition.',
  improvementNoOwner:
    'No one is kept as naming the factory definition, so no run starts. An admin names it again in team settings.',
  flawedSource: 'Marked flawed',
  lastImprovement: 'Last self-improvement run',
  noImprovement: 'No self-improvement run yet.',
  improvementFailures: {
    one: '{{count}} failure',
    other: '{{count}} failures',
  },
  openImprovement: 'Open run',
  improvementStarting: 'Starting',
  improvementOpen: 'Open',
  improvementMerged: 'Merged',
  improvementClosed: 'Closed unmerged',
  improvementRefused: 'Couldn’t start',
  // Experiments.
  experiments: 'Experiments',
  experimentsDescription:
    'Each task’s runs take one arm, and each arm gives some roles another agent',
  noExperiments: 'No experiments are set up for this workspace.',
  ended: 'Ended',
  arm: 'Arm',
  armRuns: 'Runs',
  armSucceeded: 'Succeeded',
  fellBack: {
    one: '{{count}} run fell back to another agent',
    other: '{{count}} runs fell back to another agent',
  },
  definitionAgents: 'The agent definition’s agents',
  roleAgent: '{{role}}: {{agent}}',
  // Benchmarks.
  benchmarks: 'Benchmarks',
  benchmarksDescription:
    'Frozen tasks replayed on one configuration, each trial scored with the tokens it used',
  noBenchmarks: 'No benchmarks are set up for this workspace.',
  tasksCount: { one: '{{count}} task', other: '{{count}} tasks' },
  judgedBy: 'Judged by {{scorers}}',
  noSuites: 'No suite ran in this period.',
  suiteScored: { other: '{{scored}} of {{count}} trials scored' },
  suiteInfrastructure: {
    one: '{{count}} lost to infrastructure, left out of the score',
    other: '{{count}} lost to infrastructure, left out of the score',
  },
  trial: 'Trial',
  score: 'Score',
  tokens: 'Tokens',
  scoreDefinition:
    '1 when the checks went green, 0.5 when they stayed red, 0 otherwise.',
};
