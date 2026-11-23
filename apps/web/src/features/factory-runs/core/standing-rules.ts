import type { StandingRule } from './run';

/** Each Standing Rule's name, as its translation key. */
export const RULE_NAMES = {
  approve_plan: 'factoryRuns.ruleApprovePlan',
  fix_blockers: 'factoryRuns.ruleFixBlockers',
  merge_when_green: 'factoryRuns.ruleMergeWhenGreen',
  rerun_flaky_ci: 'factoryRuns.ruleRerunFlakyCi',
} as const satisfies Record<StandingRule, string>;

/**
 * Which kinds of Gate the factory answers on its own: the plan, a review's
 * blockers, and the merge; whether the Gates it leaves go to the Run's
 * owner (the owner section); whether those Gates leave the owner
 * unnotified (`quietGates`, off when left out: the owner is notified); which the stand-in answers for the Run's
 * owner when no rule does (never the merge); and whether a Run posts its
 * updates to its task's thread (off when left out).
 */
export type OrchestratorDefinition = {
  plan: boolean;
  blockers: boolean;
  merge: boolean;
  ownerGates: boolean;
  quietGates?: boolean;
  taskThread?: boolean;
  standIn?: StandInKinds;
  /**
   * The Queue limit: the most places a person may hold (Runs waiting on
   * them at a Gate or a question, and Runs of a Flow with a Gate yet to
   * reach one); once they hold this many, no new Run of theirs starts.
   * None sets no limit.
   */
  queueLimit?: number | null;
  /**
   * The token budget: the most tokens a Run's agents may use, counted as
   * Insights counts them. Before each agent Step, a Run that used as many
   * ends blocked. A Run keeps the budget it started with. None sets none.
   */
  tokenBudget?: number | null;
  /** The kinds of Gate the Orchestrator may approve for the Run's owner. */
  orchestrator?: OrchestratorKinds;
};

/** The kinds of Gate the Orchestrator may approve: never the merge. */
export type OrchestratorKinds = {
  plan: boolean;
  blockers: boolean;
};

/** A kind of Gate a rule decides. */
export type GateKind = Exclude<
  keyof OrchestratorDefinition,
  | 'ownerGates'
  | 'quietGates'
  | 'taskThread'
  | 'standIn'
  | 'queueLimit'
  | 'tokenBudget'
  | 'orchestrator'
>;

/** The Queue limit a team starts from when it turns the limit on. */
export const DEFAULT_QUEUE_LIMIT = 5;

/** The lowest Queue limit the factory takes: lower would start no run. */
export const MIN_QUEUE_LIMIT = 1;

/** The highest Queue limit the factory takes. */
export const MAX_QUEUE_LIMIT = 1000;

/** The token budget a team starts from when it turns the budget on. */
export const DEFAULT_TOKEN_BUDGET = 5_000_000;

/** The smallest token budget the factory takes. */
export const MIN_TOKEN_BUDGET = 10_000;

/** The largest token budget the factory takes. */
export const MAX_TOKEN_BUDGET = 10_000_000_000;

/** The kinds of Gate the stand-in answers for the Run's owner. */
export type StandInKinds = {
  plan: boolean;
  blockers: boolean;
};

/** The kinds of Gate the stand-in may answer, in the Flow's order. */
export const STAND_IN_KINDS = [
  'plan',
  'blockers',
] as const satisfies readonly (keyof StandInKinds)[];

/** Each kind of Gate a rule decides, with its rule, in the Flow's order. */
export const GATE_KINDS = [
  { kind: 'plan', rule: 'approve_plan' },
  { kind: 'blockers', rule: 'fix_blockers' },
  { kind: 'merge', rule: 'merge_when_green' },
] as const satisfies readonly {
  kind: GateKind;
  rule: StandingRule;
}[];
