export default {
  title: 'Runs',
  newRun: 'New run',
  loading: 'Loading runs…',
  loadFailed: 'Runs couldn’t be loaded',
  empty: 'No runs yet',
  // Where a Run is.
  running: 'Running',
  needsInput: 'Needs input',
  starting: 'Starting',
  runningStep: 'Running {{step}}',
  waitingStep: '{{step}} needs your answer',
  // How a Run ended.
  outcomeSuccess: 'Done',
  outcomeManual: 'Handed off',
  outcomeBlocked: 'Blocked',
  outcomeFailed: 'Failed',
  outcomeCancelled: 'Cancelled',
  outcomeInfrastructure: 'Infrastructure failure',
  outcomeFlawed: 'Flawed',
  // Marking a run flawed.
  markFlawed: 'Mark flawed',
  markFlawedTitle: 'Mark this run flawed?',
  markFlawedBody:
    'Its result is wrong even though it ended done. Insights count it as flawed. A flawed mark can’t be taken back.',
  flawReasonTooLong: 'The reason must be at most {{max}} bytes.',
  flawReason: 'What’s wrong with the result',
  keepDone: 'Keep as done',
  markFlawedFailed: 'Couldn’t mark the run flawed',
  markFlawedConflict: 'Only a run that ended done can be marked flawed',
  flawed: 'Why it’s flawed',
  flawedByPerson: 'Marked flawed by its owner, {{time}}',
  flawedByScorer: 'Marked flawed by the {{scorer}} scorer, {{time}}',
  startedAt: 'Started {{time}}',
  endedAt: 'Ended {{time}}',
  // Starting a Run.
  startTitle: 'New run',
  startDescription:
    'A run takes this task through the steps of a flow and asks you when it needs a decision.',
  requestLabel: 'Request',
  requestPlaceholder: 'Describe what this run should do',
  flowLabel: 'Flow',
  flowsLoading: 'Loading flows…',
  flowsFailed: 'Flows couldn’t be loaded',
  repositoryLabel: 'Repository',
  repositoryPlaceholder: 'Search repositories',
  repositoryDescription:
    'The run opens its pull request here, with your linked GitHub account, which must be able to push to it.',
  repositoriesLoading: 'Loading repositories…',
  repositoriesFailed: 'Repositories couldn’t be loaded',
  noRepositories:
    'No repositories yet. Give Macro’s GitHub App access to the repository this run should work on.',
  showRepositories: 'Show repositories',
  connectGitHub: 'Connect GitHub',
  repositoryUnreachable:
    'Your linked GitHub account can’t reach this repository, or it has no branch yet. Connect GitHub in Settings, or choose another repository.',
  repositoryNoPush:
    'Your linked GitHub account can’t push to this repository. Choose one it can push to.',
  startRun: 'Start run',
  startFailed: 'Couldn’t start the run. Please try again.',
  queueFull:
    'Your queue is full: as many of your runs wait on you, or have yet to ask you anything, as this project’s queue limit allows. Answer some in Waiting on you, or cancel one listed there that you no longer need, then start the run.',
  openWaitingOnYou: 'Open Waiting on you',
  definitionUnreadable:
    'Your team’s factory definition doesn’t read, or your linked GitHub account can’t reach its repository. Team settings say why.',
  // A Run.
  runDetails: 'Run details',
  closeRun: 'Close run',
  runLoadFailed: 'This run couldn’t be loaded',
  request: 'Request',
  repository: 'Repository',
  repositoryFrom: '{{repository}}, from {{branch}}',
  definition: 'Factory definition',
  definitionAt: '{{repository}} at {{commit}}',
  steps: 'Steps',
  noSteps: 'No steps yet',
  stepVisit: '{{entry}} · visit {{visit}}',
  stepRunning: 'Running…',
  stepAgent: '{{role}} · {{agent}}',
  stepNoAgent: '{{role}} · no agent could start',
  passedOver: '{{agent}} couldn’t start: {{reason}}',
  passedOverUsageLimit: '{{agent}} hit its usage limit',
  showOutput: 'Show output for {{step}}',
  checksRound: 'Checks, round {{round}}',
  checksRunning: 'Checks running…',
  checksUnfinished: 'Didn’t finish',
  mainHold: 'Main hold',
  mainHoldBody:
    'Checks fail on {{branch}}, so this run won’t merge until they pass again.',
  mainHoldTask: 'See the merges since they last passed',
  baseChecksUnread:
    'Couldn’t read which checks {{branch}} requires, so this run’s merge doesn’t wait on them.',
  baseChecksNone:
    '{{branch}} has no required checks, so merges to it aren’t held.',
  reviewRound: 'Review round {{round}}',
  reviewRoundAfresh: 'Review round {{round}}, a fresh review',
  reviewRoundWhole: 'Reviews the whole change',
  reviewRoundFixDiff: 'Reviews the fix since {{commit}}',
  reviewRoundReviewing: 'Reviewing…',
  reviewRoundNotOpened: 'Couldn’t start: {{failure}}',
  reviewBlockers: { one: '{{count}} blocker', other: '{{count}} blockers' },
  reviewFollowUps: {
    one: '{{count}} follow-up',
    other: '{{count}} follow-ups',
  },
  findingBlocker: 'Blocker',
  findingFollowUp: 'Follow-up',
  findingFollowUpTask: 'Follow-up task',
  findingFound: 'Found',
  findingLine: '{{path}}, line {{line}}',
  findingLines: '{{path}}, lines {{start}}–{{end}}',
  findingWholeChange: 'The whole change',
  findingFoundAgain: 'Still there from the round before',
  checksVerified: 'Run on the pushed commit',
  checksVerifiedOn: 'Run on pushed commit {{commit}}',
  checksReported: 'Reported by the agent',
  checkExited: '{{check}} exited {{code}}',
  setupExited: 'setup {{command}} exited {{code}}',
  cancelRun: 'Cancel run',
  cancelTitle: 'Cancel this run?',
  cancelBody:
    'The step that is running stops and the run ends as cancelled. This can’t be undone.',
  keepRunning: 'Keep running',
  cancelFailed: 'Couldn’t cancel the run',
  alreadyEnded: 'This run has already ended',
  // A Gate.
  gateTitle: 'Your answer is needed',
  question: 'Question',
  gateWaiting: 'The run waits for an answer before it goes on.',
  noteLabel: 'Note',
  notePlaceholder: 'Add a note for the next steps (optional)',
  answerFailed: 'Couldn’t send your answer',
  answerConflict: 'This question was already answered',
  answerConflictDescription: 'The run has moved on and now shows where it is',
  answered: 'Answered: {{answer}}',
  answeredByRule: 'Answered by the standing rule “{{rule}}”: {{answer}}',
  answeredByStandIn: 'Answered for you by the stand-in: {{answer}}',
  answeredByOrchestrator: 'Answered by your Orchestrator: {{answer}}',
  answeredByStandingAgent:
    'Answered by agent {{agent}} as {{role}}: {{answer}}',
  answeredFromReply:
    'Answered from your reply ({{confidence}}% sure): {{answer}}',
  sendReply: 'Send as reply',
  replyFailed: 'Couldn’t send your reply',
  askedBack:
    'Couldn’t tell which answer “{{reply}}” gives. Pick an answer, or say it another way.',
  standInAnswered:
    'The stand-in answered {{answer}} for you ({{confidence}}% sure)',
  standInWindow:
    'The run goes on with it at {{time}}, unless you answer first.',
  standInDeclined: 'The stand-in left this one to you.',
  modelLeaned: 'It leaned toward {{answer}} ({{confidence}}% sure).',
  modelCouldNotTell: 'It couldn’t tell which answer fits.',
  askedBackTampered:
    'This run changed its Control Files, so approving it needs the Approve button, which lists them.',
  modelDidNotAnswer: 'It didn’t answer.',
  ruleApprovePlan: 'Approve plans',
  ruleFixBlockers: 'Fix review blockers',
  ruleMergeWhenGreen: 'Merge when CI is green',
  // An agent's question to the Run's owner.
  questionTitle: 'The {{role}} has a question',
  questionWaiting:
    'The run waits for your answer, then the agent goes on with it.',
  answerLabel: 'Your answer',
  answerPlaceholder: 'Write your answer',
  questionAnswered: 'Answered:',
  // Control Files a Run changed that its request doesn't name.
  controlFilesTitle: 'Control files changed',
  controlFilesChanged:
    'This branch changes files that decide whether its checks pass, and its request doesn’t ask for them. It merges only once you approve them.',
  controlFilesNamed: 'Named by the request:',
  controlFilesNamedLabel: 'Control files named by the request',
  controlFilesUnlisted:
    'GitHub couldn’t list this branch’s changes, so it may change files that decide whether its checks pass. It merges only once you approve that.',
  approveControlFiles: 'I approve these changes',
  // A team's Control Files, in team settings.
  controlFilesPatterns: 'Control file patterns, one a line',
  controlFilesLoading: 'Loading control files…',
  controlFilesLoadFailed: 'Control files couldn’t be loaded',
  controlFilesDefault: 'These are Macro’s default patterns.',
  controlFilesOwn: 'These are your team’s own patterns.',
  controlFilesHowTo:
    'A run whose branch changes a matching file its request doesn’t name merges only once a person approves it. `*` stays in one folder, `**` spans folders, and case is ignored.',
  controlFilesAdminsOnly: 'Only team admins can change them.',
  controlFilesUseDefault: 'Use default',
  controlFilesSave: 'Save',
  controlFilesSaveEmpty: 'Save empty list',
  controlFilesEmptyWarning:
    'An empty list checks no file: a run may then change its tests or CI unseen.',
  controlFilesSaved: 'Control files saved',
  controlFilesSaveFailed: 'Couldn’t save the control files',
  // A team's Factory Definition, in team settings.
  definitionRepository: 'Definition repository',
  definitionPlaceholder: 'https://github.com/owner/name',
  definitionLoading: 'Loading the factory definition…',
  definitionLoadFailed: 'The factory definition couldn’t be loaded',
  definitionNone:
    'Your team names no definition: runs use Macro’s built-in flows and agents, and the settings below.',
  definitionReads: 'Runs started now read it at {{commit}}.',
  definitionProblem: 'It doesn’t read: {{problem}}',
  definitionHowTo:
    'Runs take their flows, agents, rules and control files from this repository’s default branch, and keep the commit they read. Change it by pull request; the settings below no longer apply.',
  definitionNoPullRequest:
    'Its default branch doesn’t require a pull request: a push changes your factory’s rules unreviewed. Protect the branch on GitHub.',
  definitionPullRequestRequired:
    'GitHub asks for a pull request on its default branch. Anyone allowed to bypass that rule can still push to it.',
  definitionPullRequestUnknown:
    'Couldn’t tell whether its default branch requires a pull request: only its admins can read that. Make sure it does.',
  definitionAdminsOnly: 'Only team admins can change it.',
  definitionSave: 'Save',
  definitionRemove: 'Stop using it',
  definitionSaved: 'Factory definition saved',
  definitionSaveFailed: 'Couldn’t save the factory definition',
  controlFilesFromDefinition:
    'Your team’s factory definition sets the control files its runs are checked against, in its rules.json. Change them by pull request there.',
  standingRulesFromDefinition:
    'Your team’s factory definition sets the standing rules its runs start with, in its rules.json. Change them by pull request there.',
  // A team's Standing Rules, in team settings.
  standingRulesLoading: 'Loading standing rules…',
  standingRulesLoadFailed: 'Standing rules couldn’t be loaded',
  standingRulesDefault: 'These are Macro’s default rules.',
  standingRulesOwn: 'These are your team’s own rules.',
  standingRulesHowTo:
    'A run keeps the rules it started with. A gate no rule answers waits for a person.',
  standingRulesAdminsOnly: 'Only team admins can change them.',
  standingRulesApprovePlanHint: 'Approve a run’s plan without asking.',
  standingRulesFixBlockersHint:
    'Send the blockers reviewers find to be fixed, at most twice in a row; blockers found after that wait for a person.',
  standingRulesMergeWhenGreenHint:
    'Merge once CI passes and the base branch isn’t held.',
  standingRulesAlwaysPerson:
    'A run that changes control files its request doesn’t name always waits for a person.',
  standingRulesProjects: {
    one: '{{count}} project has rules of its own, which replace these for runs on its tasks.',
    other:
      '{{count}} projects have rules of their own, which replace these for runs on their tasks.',
  },
  // The person's Runs that wait on them, on the insights page.
  waitingOnYou: 'Waiting on you',
  waitingOnYouHint:
    'Runs stopped at a gate or a question only you can answer, and runs yet to ask you anything. Each holds a place in your queue.',
  holdingAPlace: 'Yet to ask you anything',
  holdingAPlaceHint:
    'Each holds the place it will ask for. Cancel one you no longer need to free its place.',
  standingRulesGateNotices: 'Notify the run’s owner when a gate waits on them',
  standingRulesGateNoticesHint:
    'On by default, for runs started from now on. Each gate that no rule or stand-in answers, and each question an agent asks, sends the run’s owner a notification that opens the run. Off, the gate still waits for them. A project’s own rules, or the rules.json of your team’s factory definition, replace this setting.',
  standingRulesOwnerGates: 'Send undecided gates to the run’s owner',
  standingRulesOwnerGatesHint:
    'Ask the run’s owner instead of fixing again when a fix leaves the blockers unchanged, and give the next step their comments on the task.',
  standingRulesTaskThread: 'Post run updates to the task',
  standingRulesTaskThreadHint:
    'Each run posts one thread on its task, as the run’s owner: its start, each step’s end, its gates and who answered them, its pull request, the merge and how it ended.',
  standingRulesUseDefault: 'Use default',
  standInTitle: 'Stand-in',
  standInPlan: 'Answer plan questions for the run’s owner',
  standInBlockers: 'Answer review blocker questions for the run’s owner',
  standInHint:
    'When no rule answers, the stand-in answers as the owner would, if it is sure enough, and says why. The owner can change its answer for ten minutes before the run goes on. It never answers a merge, a run that changes control files, or an agent’s question.',
  queueLimit: 'Limit the queue',
  queueLimitHint:
    'Once this many of a person’s runs wait on them, or have yet to ask them anything, no new run of theirs starts, whoever starts it: the person, the Orchestrator or an automation. A run that never asks anything holds no place until it does. Runs under way go on.',
  queueLimitLabel: 'Queue limit',
  queueLimitUnit: 'waiting',
  tokenBudgetSetting: 'Stop a run at a token budget',
  tokenBudgetHint:
    'Before each agent step, a run whose agents used this many tokens or more stops, and its owner is told. Tokens count as in Insights: input, output and cached input. A run keeps the budget it started with.',
  tokenBudgetLabel: 'Token budget',
  tokenBudgetUnit: 'tokens',
  orchestratorTitle: 'Orchestrator',
  orchestratorPlan: 'Let the Orchestrator approve plans you approve in chat',
  orchestratorBlockers:
    'Let the Orchestrator send review blockers to a fix you ask for in chat',
  orchestratorHint:
    'The Orchestrator is your agent in chat. It can always send work back. It never approves a merge, a run that changes control files, or blockers the rules leave to you.',
  standingRulesSaved: 'Standing rules saved',
  standingRulesSaveFailed: 'Couldn’t save the standing rules',
  // A Run an Automation started.
  startedBy: 'Started by',
  startedByAutomation: 'The automation “{{name}}”, {{trigger}}',
  tokenBudget: 'Token budget',
  tokenBudgetUsed: '{{used}} of {{budget}} tokens used',
  tokenBudgetOf: '{{budget}} tokens',
  tokenBudgetUnmeasured:
    '{{budget}} tokens, not measured: its agents record no token use, so the budget can’t stop it',
  tokenBudgetSpent:
    'Stopped at its budget: its agents used {{used}} of {{budget}} tokens',
  tokenBudgetUnread:
    'Stopped: its token use couldn’t be read, so its budget of {{budget}} tokens couldn’t be checked',
  standingActs: 'Done by agents',
  standingActStart: 'Started by agent {{agent}} as {{role}}, {{time}}',
  standingActCancel: 'Cancelled by agent {{agent}} as {{role}}, {{time}}',
  standingActLink: 'Linked to a task by agent {{agent}} as {{role}}, {{time}}',
  automationTriggerSchedule: 'on its schedule',
  automationTriggerRepositoryEvent: 'on a repository event',
  automationTriggerWebhook: 'on a call to its webhook',
  automationTriggerTaskEvent: 'on a change to the task',
  // A team's Automations, in team settings.
  automationsLoading: 'Loading automations…',
  automationsLoadFailed: 'Automations couldn’t be loaded',
  automationsEmpty: 'No automations yet.',
  automationsNoTeam: 'Automations belong to a team. Join or create one first.',
  automationsHowTo:
    'An automation starts a run as the person who made it, with their access, their team’s standing rules and control files. It starts no run on a task while one runs there.',
  automationsAdminsOnly:
    'You can switch off or delete your own automations; team admins can change any.',
  newAutomation: 'New automation',
  automationName: 'Name',
  automationTrigger: 'Starts',
  automationKindSchedule: 'Schedule',
  automationKindRepositoryEvent: 'Repository event',
  automationKindWebhook: 'Webhook',
  automationKindTaskEvent: 'Task event',
  automationCron: 'Cron, six fields',
  automationCronHint:
    'Seconds, minutes, hours, day of month, month and weekday: 0 0 9 * * Mon-Fri is 9:00 on weekdays.',
  automationTimezone: 'Time zone',
  automationEvents: 'GitHub events, comma separated',
  automationEventsPlaceholder: 'pull_request, push',
  automationActions: 'Actions, comma separated (optional)',
  automationActionsPlaceholder: 'opened',
  automationProperty: 'Property ID',
  automationPropertyHint:
    'The task property that must change: Status by default. For another property, such as tags, give its ID and the option IDs it must gain; leave those empty for any change.',
  automationValues: 'Option IDs, comma separated',
  automationStatus: 'When a task’s status becomes',
  statusNotStarted: 'Not started',
  statusInProgress: 'In progress',
  statusInReview: 'In review',
  statusCompleted: 'Completed',
  automationWebhookHint:
    'A call signed with the automation’s secret starts a run: x-macro-timestamp is the time in Unix seconds, x-macro-signature is v1= and the HMAC-SHA256 hex of “timestamp.body”. A body may name a task as {"taskId": "…"}.',
  automationRequest: 'What each run should do',
  createAutomation: 'Create automation',
  automationCreated: 'Automation created',
  automationCreateFailed: 'Couldn’t create the automation',
  automationSecretTitle: 'Webhook secret',
  automationSecretHint:
    'Copy it now: it isn’t shown again. Calls go to this address.',
  automationEnabled: 'On',
  deleteAutomation: 'Delete',
  automationChangeFailed: 'Couldn’t change the automation',
  automationSummarySchedule: '{{cron}} ({{timezone}})',
  automationSummaryRepositoryEvent: '{{events}} on {{repository}}',
  automationSummaryWebhook: 'Signed webhook calls',
  automationSummaryTaskEvent: 'A task’s property gains a value',
  // A running Step's Step Terminal: its agent's session, watched and steered.
  watchStep: 'Watch',
  watchStepLabel: 'Watch {{step}} and steer its agent',
  stepPaused: 'Paused',
  stepStopping: 'Stopping',
  terminalTitle: '{{step}}: the agent at work',
  closeTerminal: 'Close',
  terminalEnded: 'This step has ended. Its agent’s session is closed.',
  terminalPausedHint:
    'The agent is paused. Resume it to go on; a note you write goes with the resume.',
  terminalPausedEndsBy:
    'Its time runs out at {{time}}: unless you resume it by then, it ends as failed.',
  terminalPauseRanOut:
    'The step stayed paused past its time, and ended as failed.',
  terminalBetweenTurns:
    'The agent is between turns while the factory runs the repository’s checks. You can steer it again once its next turn starts.',
  terminalStoppingHint: 'The step is stopping, and will end as failed.',
  terminalNoteLabel: 'Note to the agent',
  terminalNotePlaceholder:
    'Write the agent a note; it reads it once its current turn ends',
  sendNote: 'Send note',
  pauseStep: 'Pause',
  resumeStep: 'Resume',
  stopStep: 'Stop step',
  stopStepTitle: 'Stop this step?',
  stopStepBody:
    'The agent stops and the step ends as failed; the run goes on as its flow says.',
  keepStepRunning: 'Keep it running',
  steerConflict: 'The step can’t take that now',
  steerConflictDescription:
    'It ended, is paused, or its agent is between turns. The run shows where it is.',
  steerFailed: 'Couldn’t reach the step',
  // What the owner did to a Step, under it.
  steerNote: 'You sent the agent a note',
  steerPause: 'You paused the step',
  steerResume: 'You resumed the step',
  steerStop: 'You stopped the step',
  steerUndelivered: 'The agent never got it',
  // An agent session a factory Step works in, opened on its own.
  stepSessionNotice:
    'A factory step works in this session. Steer it from its run, which keeps what you do.',
  stepSessionOpenRun: 'Open the run',
};
