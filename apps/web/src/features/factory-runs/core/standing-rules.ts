import type { StandingRule } from './run';

/** Each Standing Rule's name, as its translation key. */
export const RULE_NAMES = {
  approve_plan: 'factoryRuns.ruleApprovePlan',
  fix_blockers: 'factoryRuns.ruleFixBlockers',
  merge_when_green: 'factoryRuns.ruleMergeWhenGreen',
} as const satisfies Record<StandingRule, string>;

/**
 * Which kinds of Gate the factory answers on its own: the plan, a review's
 * blockers, and the merge; whether the Gates it leaves go to the Run's
 * owner (the owner section); which the stand-in answers for the Run's
 * owner when no rule does (never the merge); and whether a Run posts its
 * updates to its task's thread (off when left out).
 */
export type OrchestratorDefinition = {
  plan: boolean;
  blockers: boolean;
  merge: boolean;
  ownerGates: boolean;
  taskThread?: boolean;
  standIn?: StandInKinds;
  /**
   * The Queue limit: above this many places held by a person (Runs waiting
   * on them at a Gate or a question, and Runs of a Flow with a Gate yet to
   * reach one), no new Run of theirs starts. None sets no limit.
   */
  queueLimit?: number | null;
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
  'ownerGates' | 'taskThread' | 'standIn' | 'queueLimit' | 'orchestrator'
>;

/** The Queue limit a team starts from when it turns the limit on. */
export const DEFAULT_QUEUE_LIMIT = 5;

/** The highest Queue limit the factory takes. */
export const MAX_QUEUE_LIMIT = 1000;

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
