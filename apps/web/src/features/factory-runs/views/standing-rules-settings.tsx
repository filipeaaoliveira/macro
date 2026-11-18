import { t } from '@app/lib/i18n';
import { toast } from '@core/component/Toast/Toast';
import { fireAndForget } from '@core/util/fireAndForget';
import { ThrownResultError } from '@core/util/result';
import { Button, Checkbox } from '@ui';
import { For, Match, Show, Switch } from 'solid-js';
import {
  GATE_KINDS,
  type GateKind,
  type OrchestratorDefinition,
  RULE_NAMES,
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

/**
 * The team's Standing Rules: which kinds of Gate the factory answers on its
 * own for its Runs, and whether the Gates it leaves go to the Run's owner.
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
      <Match when={query.data}>
        {(settings) => {
          const locked = () => !settings().canEdit || mutation.isPending;
          // A definition read without the owner section has it off.
          const definition = (): OrchestratorDefinition => ({
            ...settings().definition,
            ownerGates: settings().definition.ownerGates ?? false,
          });
          const toggle = (kind: keyof OrchestratorDefinition, on: boolean) =>
            fireAndForget(
              save({ ...definition(), [kind]: on }),
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
