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
 * owner (the owner section); and which the stand-in answers for the Run's
 * owner when no rule does (never the merge).
 */
export type OrchestratorDefinition = {
  plan: boolean;
  blockers: boolean;
  merge: boolean;
  ownerGates: boolean;
  standIn?: StandInKinds;
};

/** A kind of Gate a rule decides. */
export type GateKind = Exclude<keyof OrchestratorDefinition, 
  'ownerGates' | 'standIn'
>;

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
