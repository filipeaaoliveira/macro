import { t } from '@app/lib/i18n';
import { toast } from '@core/component/Toast/Toast';
import { fireAndForget } from '@core/util/fireAndForget';
import { ThrownResultError } from '@core/util/result';
import { Button, Checkbox } from '@ui';
import { For, Match, Show, Switch } from 'solid-js';
import {
  GATE_KINDS,
  type OrchestratorDefinition,
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
} as const satisfies Record<
  Exclude<keyof OrchestratorDefinition, 'standIn'>,
  string
>;

const STAND_IN_NAMES = {
  plan: 'factoryRuns.standInPlan',
  blockers: 'factoryRuns.standInBlockers',
} as const satisfies Record<keyof StandInKinds, string>;

/** The stand-in is off for every kind a definition leaves out. */
const NO_STAND_IN: StandInKinds = { plan: false, blockers: false };

/**
 * The team's Standing Rules: which kinds of Gate the factory answers on its
 * own for its Runs. Its admins turn each on or off, or go back to the
 * default; other members read them. A Tampered Run always waits for a
 * person, whatever they say.
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
      <Match when={query.data}>
        {(settings) => {
          const locked = () => !settings().canEdit || mutation.isPending;
          const toggle = (
            kind: Exclude<keyof OrchestratorDefinition, 'standIn'>,
            on: boolean
          ) =>
            fireAndForget(
              save({ ...settings().definition, [kind]: on }),
              'factory-runs'
            );
          const standIn = () => settings().definition.standIn ?? NO_STAND_IN;
          const toggleStandIn = (kind: keyof StandInKinds, on: boolean) =>
            fireAndForget(
              save({
                ...settings().definition,
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
