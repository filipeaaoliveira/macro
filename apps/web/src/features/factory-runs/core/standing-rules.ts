import type { StandingRule } from './run';

/** Each Standing Rule's name, as its translation key. */
export const RULE_NAMES = {
  approve_plan: 'factoryRuns.ruleApprovePlan',
  fix_blockers: 'factoryRuns.ruleFixBlockers',
  merge_when_green: 'factoryRuns.ruleMergeWhenGreen',
} as const satisfies Record<StandingRule, string>;

/**
 * Which kinds of Gate the factory answers on its own: the plan, a review's
 * blockers, and the merge.
 */
export type OrchestratorDefinition = {
  plan: boolean;
  blockers: boolean;
  merge: boolean;
};

/** Each kind of Gate a rule decides, with its rule, in the Flow's order. */
export const GATE_KINDS = [
  { kind: 'plan', rule: 'approve_plan' },
  { kind: 'blockers', rule: 'fix_blockers' },
  { kind: 'merge', rule: 'merge_when_green' },
] as const satisfies readonly {
  kind: keyof OrchestratorDefinition;
  rule: StandingRule;
}[];
