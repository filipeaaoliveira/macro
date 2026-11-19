import { t } from '@app/lib/i18n';
import { toast } from '@core/component/Toast/Toast';
import { fireAndForget } from '@core/util/fireAndForget';
import { ThrownResultError } from '@core/util/result';
import { Button, Checkbox, NumberUnitInput } from '@ui';
import { createSignal, For, Match, Show, Switch } from 'solid-js';
import {
  DEFAULT_QUEUE_LIMIT,
  DEFAULT_TOKEN_BUDGET,
  GATE_KINDS,
  type GateKind,
  MAX_QUEUE_LIMIT,
  MAX_TOKEN_BUDGET,
  MIN_QUEUE_LIMIT,
  MIN_TOKEN_BUDGET,
  type OrchestratorDefinition,
  type OrchestratorKinds,
  RULE_NAMES,
  STAND_IN_KINDS,
  type StandInKinds,
} from '../core/standing-rules';
import {
  useSetStandingRulesMutation,
  useStandingRulesQuery,
} from '../queries/standing-rules';

const RULE_HINTS = {
  plan: 'factoryRuns.standingRulesApprovePlanHint',
  blockers: 'factoryRuns.standingRulesFixBlockersHint',
  merge: 'factoryRuns.standingRulesMergeWhenGreenHint',
} as const satisfies Record<GateKind, string>;

const STAND_IN_NAMES = {
  plan: 'factoryRuns.standInPlan',
  blockers: 'factoryRuns.standInBlockers',
} as const satisfies Record<keyof StandInKinds, string>;

const ORCHESTRATOR_NAMES = {
  plan: 'factoryRuns.orchestratorPlan',
  blockers: 'factoryRuns.orchestratorBlockers',
} as const satisfies Record<keyof OrchestratorKinds, string>;

/** The Orchestrator approves no kind a definition leaves out. */
const NO_ORCHESTRATOR: OrchestratorKinds = { plan: false, blockers: false };

/** The stand-in is off for every kind a definition leaves out. */
const NO_STAND_IN: StandInKinds = { plan: false, blockers: false };

/**
 * The team's Standing Rules: which kinds of Gate the factory answers on its
 * own for its Runs, whether the Gates it leaves go to the Run's owner,
 * whether its Runs post their updates to their task's thread, its Queue
 * limit, its token budget, which the stand-in answers for the owner when no rule does, and
 * which the Orchestrator may approve.
 * Its admins turn each on or off, or go back to the default; other members
 * read them. A Tampered Run always waits for a person, whatever they say.
 */
export function StandingRulesSettings() {
  const query = useStandingRulesQuery();
  const mutation = useSetStandingRulesMutation();

  const save = async (definition: OrchestratorDefinition | null) => {
    try {
      await mutation.mutateAsync(definition);
      toast.success(t('factoryRuns.standingRulesSaved'));
    } catch (error) {
      if (error instanceof ThrownResultError) {
        toast.alert(t('factoryRuns.standingRulesSaveFailed'), {
          subtext: error.message,
        });
        return;
      }
      toast.failure(t('factoryRuns.standingRulesSaveFailed'));
    }
  };

  return (
    <Switch>
      <Match when={query.isPending}>
        <p class="p-3 text-sm text-ink-muted" role="status">
          {t('factoryRuns.standingRulesLoading')}
        </p>
      </Match>
      <Match when={query.isError}>
        <div class="flex items-center gap-2 p-3">
          <p class="text-sm text-ink-muted">
            {t('factoryRuns.standingRulesLoadFailed')}
          </p>
          <Button variant="ghost" size="xs" onClick={() => query.refetch()}>
            {t('common.retry')}
          </Button>
        </div>
      </Match>
      <Match when={query.data?.definitionNamed}>
        <p class="p-3 text-sm text-ink-muted">
          {t('factoryRuns.standingRulesFromDefinition')}
        </p>
      </Match>
      <Match when={query.data}>
        {(settings) => {
          const locked = () => !settings().canEdit || mutation.isPending;
          // A definition read without the owner section has it off, and
          // one read without `quietGates` notifies the run's owner.
          const definition = (): OrchestratorDefinition => ({
            ...settings().definition,
            ownerGates: settings().definition.ownerGates ?? false,
          });
          const toggle = (
            kind: Exclude<
              keyof OrchestratorDefinition,
              'standIn' | 'queueLimit' | 'tokenBudget' | 'orchestrator'
            >,
            on: boolean
          ) =>
            fireAndForget(
              save({ ...definition(), [kind]: on }),
              'factory-runs'
            );
          const standIn = () => definition().standIn ?? NO_STAND_IN;
          const orchestrator = () =>
            definition().orchestrator ?? NO_ORCHESTRATOR;
          const toggleOrchestrator = (
            kind: keyof OrchestratorKinds,
            on: boolean
          ) =>
            fireAndForget(
              save({
                ...definition(),
                orchestrator: { ...orchestrator(), [kind]: on },
              }),
              'factory-runs'
            );
          const queueLimit = () => definition().queueLimit ?? null;
          const setQueueLimit = (limit: number | null) =>
            fireAndForget(
              save({ ...definition(), queueLimit: limit }),
              'factory-runs'
            );
          // The limit typed so far, saved when the field loses focus.
          const [typedLimit, setTypedLimit] = createSignal<number>();
          const saveTypedLimit = () => {
            const typed = typedLimit();
            setTypedLimit(undefined);
            if (typed === undefined || Number.isNaN(typed)) return;
            const limit = Math.min(
              Math.max(Math.round(typed), MIN_QUEUE_LIMIT),
              MAX_QUEUE_LIMIT
            );
            if (limit !== queueLimit()) setQueueLimit(limit);
          };
          const tokenBudget = () => definition().tokenBudget ?? null;
          const setTokenBudget = (budget: number | null) =>
            fireAndForget(
              save({ ...definition(), tokenBudget: budget }),
              'factory-runs'
            );
          // The budget typed so far, saved when the field loses focus.
          const [typedBudget, setTypedBudget] = createSignal<number>();
          const saveTypedBudget = () => {
            const typed = typedBudget();
            setTypedBudget(undefined);
            if (typed === undefined || Number.isNaN(typed)) return;
            const budget = Math.min(
              Math.max(Math.round(typed), MIN_TOKEN_BUDGET),
              MAX_TOKEN_BUDGET
            );
            if (budget !== tokenBudget()) setTokenBudget(budget);
          };
          const toggleStandIn = (kind: keyof StandInKinds, on: boolean) =>
            fireAndForget(
              save({
                ...definition(),
                standIn: { ...standIn(), [kind]: on },
              }),
              'factory-runs'
            );
          return (
            <div class="flex flex-col gap-3 p-3">
              <For each={GATE_KINDS}>
                {(gate) => (
                  <Checkbox
                    size="sm"
                    checked={settings().definition[gate.kind]}
                    onChange={(on: boolean) => toggle(gate.kind, on)}
                    disabled={locked()}
                  >
                    <Checkbox.Control />
                    <div class="flex flex-col gap-0.5">
                      <Checkbox.Label class="text-sm">
                        {t(RULE_NAMES[gate.rule])}
                      </Checkbox.Label>
                      <Checkbox.Description class="text-xs text-ink-muted">
                        {t(RULE_HINTS[gate.kind])}
                      </Checkbox.Description>
                    </div>
                  </Checkbox>
                )}
              </For>
              <Checkbox
                size="sm"
                checked={!(definition().quietGates ?? false)}
                onChange={(on: boolean) => toggle('quietGates', !on)}
                disabled={locked()}
              >
                <Checkbox.Control />
                <div class="flex flex-col gap-0.5">
                  <Checkbox.Label class="text-sm">
                    {t('factoryRuns.standingRulesGateNotices')}
                  </Checkbox.Label>
                  <Checkbox.Description class="text-xs text-ink-muted">
                    {t('factoryRuns.standingRulesGateNoticesHint')}
                  </Checkbox.Description>
                </div>
              </Checkbox>
              <Checkbox
                size="sm"
                checked={definition().ownerGates}
                onChange={(on: boolean) => toggle('ownerGates', on)}
                disabled={locked()}
              >
                <Checkbox.Control />
                <div class="flex flex-col gap-0.5">
                  <Checkbox.Label class="text-sm">
                    {t('factoryRuns.standingRulesOwnerGates')}
                  </Checkbox.Label>
                  <Checkbox.Description class="text-xs text-ink-muted">
                    {t('factoryRuns.standingRulesOwnerGatesHint')}
                  </Checkbox.Description>
                </div>
              </Checkbox>
              <Checkbox
                size="sm"
                checked={definition().taskThread ?? false}
                onChange={(on: boolean) => toggle('taskThread', on)}
                disabled={locked()}
              >
                <Checkbox.Control />
                <div class="flex flex-col gap-0.5">
                  <Checkbox.Label class="text-sm">
                    {t('factoryRuns.standingRulesTaskThread')}
                  </Checkbox.Label>
                  <Checkbox.Description class="text-xs text-ink-muted">
                    {t('factoryRuns.standingRulesTaskThreadHint')}
                  </Checkbox.Description>
                </div>
              </Checkbox>
              <Checkbox
                size="sm"
                checked={queueLimit() !== null}
                onChange={(on: boolean) =>
                  setQueueLimit(on ? DEFAULT_QUEUE_LIMIT : null)
                }
                disabled={locked()}
              >
                <Checkbox.Control />
                <div class="flex flex-col gap-0.5">
                  <Checkbox.Label class="text-sm">
                    {t('factoryRuns.queueLimit')}
                  </Checkbox.Label>
                  <Checkbox.Description class="text-xs text-ink-muted">
                    {t('factoryRuns.queueLimitHint')}
                  </Checkbox.Description>
                </div>
              </Checkbox>
              <Show when={queueLimit() !== null}>
                <NumberUnitInput
                  class="pl-6"
                  format={false}
                  rawValue={typedLimit() ?? queueLimit() ?? undefined}
                  minValue={MIN_QUEUE_LIMIT}
                  maxValue={MAX_QUEUE_LIMIT}
                  step={1}
                  disabled={locked()}
                  onRawValueChange={setTypedLimit}
                >
                  <NumberUnitInput.Frame>
                    <NumberUnitInput.Input
                      aria-label={t('factoryRuns.queueLimitLabel')}
                      onBlur={saveTypedLimit}
                    />
                    <NumberUnitInput.Unit>
                      {t('factoryRuns.queueLimitUnit')}
                    </NumberUnitInput.Unit>
                  </NumberUnitInput.Frame>
                </NumberUnitInput>
              </Show>
              <Checkbox
                size="sm"
                checked={tokenBudget() !== null}
                onChange={(on: boolean) =>
                  setTokenBudget(on ? DEFAULT_TOKEN_BUDGET : null)
                }
                disabled={locked()}
              >
                <Checkbox.Control />
                <div class="flex flex-col gap-0.5">
                  <Checkbox.Label class="text-sm">
                    {t('factoryRuns.tokenBudgetSetting')}
                  </Checkbox.Label>
                  <Checkbox.Description class="text-xs text-ink-muted">
                    {t('factoryRuns.tokenBudgetHint')}
                  </Checkbox.Description>
                </div>
              </Checkbox>
              <Show when={tokenBudget() !== null}>
                <NumberUnitInput
                  class="pl-6"
                  format={false}
                  rawValue={typedBudget() ?? tokenBudget() ?? undefined}
                  minValue={MIN_TOKEN_BUDGET}
                  maxValue={MAX_TOKEN_BUDGET}
                  step={1}
                  disabled={locked()}
                  onRawValueChange={setTypedBudget}
                >
                  <NumberUnitInput.Frame>
                    <NumberUnitInput.Input
                      aria-label={t('factoryRuns.tokenBudgetLabel')}
                      onBlur={saveTypedBudget}
                    />
                    <NumberUnitInput.Unit>
                      {t('factoryRuns.tokenBudgetUnit')}
                    </NumberUnitInput.Unit>
                  </NumberUnitInput.Frame>
                </NumberUnitInput>
              </Show>
              <p class="pt-1 text-sm font-medium">
                {t('factoryRuns.standInTitle')}
              </p>
              <For each={STAND_IN_KINDS}>
                {(kind) => (
                  <Checkbox
                    size="sm"
                    checked={standIn()[kind]}
                    onChange={(on: boolean) => toggleStandIn(kind, on)}
                    disabled={locked()}
                  >
                    <Checkbox.Control />
                    <Checkbox.Label class="text-sm">
                      {t(STAND_IN_NAMES[kind])}
                    </Checkbox.Label>
                  </Checkbox>
                )}
              </For>
              <p class="text-xs text-ink-muted">
                {t('factoryRuns.standInHint')}
              </p>
              <p class="pt-1 text-sm font-medium">
                {t('factoryRuns.orchestratorTitle')}
              </p>
              <For each={STAND_IN_KINDS}>
                {(kind) => (
                  <Checkbox
                    size="sm"
                    checked={orchestrator()[kind]}
                    onChange={(on: boolean) => toggleOrchestrator(kind, on)}
                    disabled={locked()}
                  >
                    <Checkbox.Control />
                    <Checkbox.Label class="text-sm">
                      {t(ORCHESTRATOR_NAMES[kind])}
                    </Checkbox.Label>
                  </Checkbox>
                )}
              </For>
              <p class="text-xs text-ink-muted">
                {t('factoryRuns.orchestratorHint')}
              </p>
              <p class="text-xs text-ink-muted">
                {settings().isDefault
                  ? t('factoryRuns.standingRulesDefault')
                  : t('factoryRuns.standingRulesOwn')}{' '}
                {t('factoryRuns.standingRulesHowTo')}{' '}
                {t('factoryRuns.standingRulesAlwaysPerson')}
              </p>
              <Show when={settings().projects.length > 0}>
                <p class="text-xs text-ink-muted">
                  {t('factoryRuns.standingRulesProjects', {
                    count: settings().projects.length,
                  })}
                </p>
              </Show>
              <Show
                when={settings().canEdit}
                fallback={
                  <p class="text-xs text-ink-muted">
                    {t('factoryRuns.standingRulesAdminsOnly')}
                  </p>
                }
              >
                <div class="flex justify-end">
                  <Button
                    variant="secondary"
                    size="sm"
                    disabled={locked() || settings().isDefault}
                    onClick={() => fireAndForget(save(null), 'factory-runs')}
                  >
                    {t('factoryRuns.standingRulesUseDefault')}
                  </Button>
                </div>
              </Show>
            </div>
          );
        }}
      </Match>
    </Switch>
  );
}
