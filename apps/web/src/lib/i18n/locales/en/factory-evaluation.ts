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
  agreement: 'Agrees with you',
  labelled: {
    one: '{{count}} label',
    other: '{{count}} labels',
  },
  calibrated: 'Calibrated',
  uncalibrated: 'Not calibrated',
  uncalibratedMark: '(not calibrated)',
  calibratedLine: {
    one: 'Its judge agrees with {{agreement}} of your {{count}} label, so its results can be trusted.',
    other:
      'Its judge agrees with {{agreement}} of your {{count}} labels, so its results can be trusted.',
  },
  needsLabelsLine: {
    one: 'Not calibrated yet: label {{count}} more of its judgments from a run’s view.',
    other:
      'Not calibrated yet: label {{count}} more of its judgments from a run’s view.',
  },
  disagreesLine:
    'Not calibrated: its judge agrees with {{agreement}} of your labels, and needs {{threshold}}.',
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
  improvementOff:
    'Turned off in the factory definition: failures are still kept, and no run starts.',
  improvementNoOwner:
    'No one is kept as naming the factory definition, so no run starts. An admin names it again in team settings.',
  improvementNamerLeft:
    'The admin who named the factory definition left the team, so no run starts. An admin names it again in team settings.',
  improvementNamerNotAdmin:
    'The person who named the factory definition is no longer a team admin, so no run starts. An admin names it again in team settings.',
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
  improvementDismissed: {
    one: '{{count}} failure dismissed by the rejection',
    other: '{{count}} failures dismissed by the rejection',
  },
  dismissalUndone: 'Dismissal undone {{time}}: its failures count again',
  undoDismissal: 'Undo dismissal',
  undoDismissalTitle: 'Count these failures again?',
  undoDismissalBody:
    'If the fix was rejected, not the failures, they go back to pending and count toward the next self-improvement run once this rejection’s pause ends.',
  keepDismissed: 'Keep dismissed',
  undoDismissalConflict: 'These failures are no longer dismissed.',
  undoDismissalFailed: 'Couldn’t undo the dismissal. Try again.',
  undoDismissalForbidden: 'You may no longer undo this rejection.',
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
  // Agent Rules.
  agentRules: 'Agent rules',
  agentRulesDescription:
    'Runs whose task tag or repository a rule matched, and the agent it chose for a role',
  agentRule: 'Rule',
  agentRuleChose: '{{role}}: {{agent}}',
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
  paretoSet: 'Pareto set',
  tokensPerTrial: 'Tokens per trial',
  recommendedFor: 'Recommended for {{benchmark}}',
  paretoDescription:
    'A suite is on the Pareto set when no other suite of its benchmark scores at least as well for no more tokens per trial. The recommended suite has the best score on the set.',
  scoreDefinition:
    '1 when the checks went green, 0.5 when they stayed red, 0 otherwise.',
};
