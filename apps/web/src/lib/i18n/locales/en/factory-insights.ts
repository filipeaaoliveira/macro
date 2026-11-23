export default {
  title: 'Run insights',
  actions: 'Insights actions',
  loading: 'Loading run insights…',
  loadFailed: 'Run insights couldn’t be loaded',
  // Filters.
  filters: 'Insights filters',
  flow: 'Flow',
  allFlows: 'All flows',
  period: 'Period',
  lastDays: { other: 'Last {{count}} days' },
  periodLine: '{{from}} – {{to}} · {{timeZone}}',
  capped: { other: 'Showing the newest {{count}} runs of this period.' },
  empty:
    'No runs match this period and flow. Try a longer period or another flow.',
  footnote:
    'Runs count on the day they were created. A run is counted once, by how it ended or as running. Times are medians and the slowest tenth of runs or steps; cancelled runs are left out of times.',
  // Runs.
  runs: 'Runs',
  totalRuns: 'Total runs',
  totalRunsDefinition: 'Runs you started in this period, of the chosen flow.',
  successRate: 'Success rate',
  successRateDefinition:
    'Runs that reached their flow’s end and weren’t found flawed, of those that ended on their own. Runs still going, runs you cancelled and runs that failed for infrastructure are left out.',
  // Quality apart from plumbing.
  outcomeKinds: 'Quality and infrastructure',
  outcomeKindsDescription:
    'Failures of the work apart from failures of the factory’s machines, quota or network, and results found wrong after the run succeeded.',
  flawedRate: 'Flawed',
  flawedRateDefinition:
    'Runs that reached their flow’s end but whose result a scorer or a person marked flawed, of all runs that reached their end.',
  infrastructureRate: 'Infrastructure failures',
  infrastructureRateDefinition:
    'Runs that failed for a machine, quota or network reason rather than their work, of those that ended and weren’t cancelled.',
  failedForWork: 'Failed for the work',
  failedForInfrastructure: 'Failed for infrastructure',
  flawedRuns: 'Flawed runs',
  medianRunTime: 'Median run time',
  medianRunTimeDefinition:
    'Half of the runs that ended took less than this, from start to end, waiting for your answers included.',
  slowRunTime: 'Slowest runs',
  slowRunTimeDefinition:
    'A tenth of the runs that ended took longer than this, from start to end.',
  outcomes: 'Run outcomes',
  outcomesChart: 'Run outcomes donut chart',
  outcomeShare: '{{outcome}}: {{count}} ({{share}})',
  oneStatePerRun: 'Each run appears in one state.',
  retriedRuns: {
    one: '{{count}} of these runs was retried after it ended. Its retry is a run of its own, counted by how it went.',
    other:
      '{{count}} of these runs were retried after they ended. Each retry is a run of its own, counted by how it went.',
  },
  noRunsInPeriod: 'No runs in this period.',
  showCounts: 'Show counts',
  showPercentages: 'Show percentages',
  runsByDay: 'Runs by day',
  runsByDayChart: 'Runs started each day, by how they went',
  runsByDayExplore:
    'Runs by day chart. Use the left and right arrow keys to explore the days.',
  visibleStates: 'Visible run states',
  viewDailyData: 'View daily data',
  date: 'Date',
  runsByFlow: 'Runs by flow',
  runsByFlowDescription: 'Number of runs of each flow',
  unrecordedFlow: 'Earlier runs',
  // Steps.
  steps: 'Steps',
  stepsDescription:
    'Each step of your flows, in order: how long it took, how often a run came back to it, and how it ended.',
  noSteps: 'No steps in this period.',
  stepsCount: { one: '{{count}} step', other: '{{count}} steps' },
  runsCount: { one: '{{count}} run', other: '{{count}} runs' },
  medianTime: 'Median time',
  slowestTime: 'Slowest tenth',
  wentAgain: 'Went again in',
  gateWait: 'Wait for answer',
  tokens: 'Tokens',
  stepOutcomes: 'How the steps ended',
  gateAnswers: 'Answers given',
  // Where Runs stop.
  whereRunsStop: 'Where runs stop',
  whereRunsStopDescription:
    'Steps where runs failed or went back, counted in runs, with the latest of those runs',
  nothingStopped: 'No step failed or went again in this period.',
  failedRuns: { one: '{{count}} run failed', other: '{{count}} runs failed' },
  againRuns: {
    one: '{{count}} run went again',
    other: '{{count}} runs went again',
  },
  runsAt: 'Runs at {{step}}',
  failed: 'Failed',
  visits: { other: '{{count}} visits' },
  // Agent usage.
  agentUsage: 'Agent usage',
  agentUsageDescription: 'Tokens the agents used for your runs’ steps',
  tokensDefinition:
    'Input, output and prompt cache tokens of the agents’ model calls, as Macro recorded them.',
  tokensPerRun: 'Tokens per run',
  tokensPerRunDefinition: 'Tokens per run, among the runs with usage recorded.',
  measuredSteps: {
    one: 'Usage recorded for {{measured}} of {{count}} agent step. Agents on your own computer record none.',
    other:
      'Usage recorded for {{measured}} of {{count}} agent steps. Agents on your own computer record none.',
  },
  heaviestRuns: 'Runs that used the most tokens',
};
