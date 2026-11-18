export default {
  title: 'Factory',
  actions: 'Factory actions',
  scope: 'Whose runs',
  scopeMine: 'Mine',
  scopeTeam: 'My team',
  search: 'Search runs by task',
  states: 'Where runs are',
  allStates: 'All',
  // Where a Run is, as the board groups it.
  stateRunning: 'Running',
  stateWaiting: 'Waiting on a person',
  stateHeld: 'Held',
  stateStopped: 'Stopped for a person',
  stateFinished: 'Finished',
  // Filters.
  filters: 'Filters',
  flow: 'Flow',
  allFlows: 'All flows',
  repository: 'Repository',
  allRepositories: 'All repositories',
  project: 'Project',
  allProjects: 'All projects',
  // A row.
  you: 'you',
  someone: 'A teammate',
  startedByPerson: 'Started by {{owner}}',
  startedByOrchestrator: 'Started by the Orchestrator for {{owner}}',
  startedByAutomation: 'Started by automation {{name}}',
  waitingOn: 'Waiting on {{owner}}',
  heldBy: 'Held: {{branch}} is red',
  stepWithAgent: '{{step}} · {{role}} ({{agent}})',
  stepWithRole: '{{step}} · {{role}}',
  blockers: { one: '{{count}} blocker', other: '{{count}} blockers' },
  pullRequest: 'Pull request',
  // The capacity header.
  queue: 'Your queue',
  queueHeld: {
    one: '{{count}} of {{limit}} places',
    other: '{{count}} of {{limit}} places',
  },
  queueUnlimited: {
    one: '{{count}} place, no limit',
    other: '{{count}} places, no limit',
  },
  waitingOnYou: 'Waiting on you',
  // The list.
  loadFailed: 'Runs couldn’t be loaded',
  empty: 'No runs here',
  emptyHint:
    'Runs you start from a task show here while they go and once they end.',
  emptyHintTeam:
    'Runs you and your team start show here, on tasks you can see.',
  // A group's count while more Runs follow than are loaded.
  loadedCount: '{{count}}+',
  loadMore: 'Show more',
  stoppedEarly:
    'Older runs may be missing: the board stopped past many runs you can’t see.',
  // A pull request a Run opened, on the Reviews page.
  pullRequestRun: 'Factory run',
  pullRequestMark: 'Factory',
  openRun: 'Open run',
  waitingToMerge: 'Waiting on {{owner}} to merge',
  reviewRound: 'Review round {{round}}',
  mergeAtGate: 'Merging answers its run’s merge gate',
  answerMergeGate: 'Answer merge gate',
  mergeOnlyOwner: 'Only {{owner}} can merge this, at its run’s merge gate',
  mergeOnlyTheOwner: 'Only its run’s owner can merge this, at its merge gate',
  mergeHeld: 'Merges once {{branch}} is green again',
  mergeNotAtGate: 'Merges at its run’s merge gate, once the run gets there',
  mergeNotAtGateStep:
    'Merges at its run’s merge gate, once the run gets there: it’s at {{step}}',
  noReviewRounds: 'No review rounds yet',
  mergeChecking: 'Checking for a factory run…',
  mergeUnguarded: 'Open this pull request in Macro’s Reviews to merge it',
  mergeAfterOtherGate:
    'Merges at its run’s merge gate: the run waits at {{step}} first',
  mergeRunStopped:
    'Its run stopped ({{outcome}}): merging skips its merge gate and the checks it held',
};
