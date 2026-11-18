import { repositoryLabel } from '@app/features/agents-view/core/repository';
import {
  PROPERTY_OPTION_IDS,
  SYSTEM_PROPERTY_IDS,
} from '@app/features/property/identifiers';
import { t } from '@app/lib/i18n';
import { toast } from '@core/component/Toast/Toast';
import { SERVER_HOSTS } from '@core/constant/servers';
import { fireAndForget } from '@core/util/fireAndForget';
import { ThrownResultError } from '@core/util/result';
import { useAgentRepositoriesQuery } from '@queries/agent-repositories/repositories';
import type {
  Automation,
  AutomationTrigger,
  CreatedAutomation,
  TriggerKind,
} from '@service-factory/generated/schemas';
import { useQuery } from '@tanstack/solid-query';
import { Button, Checkbox, RadioGroup, Tabs, TextField } from '@ui';
import { createSignal, For, Match, Show, Switch } from 'solid-js';
import { match } from 'ts-pattern';
import { RepositoryField } from '../components/repository-field';
import { flowWordLabel } from '../core/flow-words';
import {
  useAutomationMutations,
  useAutomationsQuery,
} from '../queries/automations';
import { flowsOptions } from '../queries/runs';

const TRIGGER_KINDS = [
  { value: 'schedule', label: 'factoryRuns.automationKindSchedule' },
  {
    value: 'repository_event',
    label: 'factoryRuns.automationKindRepositoryEvent',
  },
  { value: 'webhook', label: 'factoryRuns.automationKindWebhook' },
  { value: 'task_event', label: 'factoryRuns.automationKindTaskEvent' },
] as const satisfies readonly { value: TriggerKind; label: string }[];

const STATUSES = [
  {
    value: PROPERTY_OPTION_IDS.STATUS.NOT_STARTED,
    label: 'factoryRuns.statusNotStarted',
  },
  {
    value: PROPERTY_OPTION_IDS.STATUS.IN_PROGRESS,
    label: 'factoryRuns.statusInProgress',
  },
  {
    value: PROPERTY_OPTION_IDS.STATUS.IN_REVIEW,
    label: 'factoryRuns.statusInReview',
  },
  {
    value: PROPERTY_OPTION_IDS.STATUS.COMPLETED,
    label: 'factoryRuns.statusCompleted',
  },
] as const;

/** The words of a comma-separated list, blanks left out. */
function wordsOf(text: string): string[] {
  return text
    .split(',')
    .map((word) => word.trim())
    .filter((word) => word.length > 0);
}

/** What starts an Automation's Runs, in a line. */
function triggerSummary(automation: Automation): string {
  const trigger = automation.trigger;
  return match(trigger)
    .with({ kind: 'schedule' }, ({ cron, timezone }) =>
      t('factoryRuns.automationSummarySchedule', { cron, timezone })
    )
    .with({ kind: 'repository_event' }, ({ events, actions }) =>
      t('factoryRuns.automationSummaryRepositoryEvent', {
        events: [...events, ...(actions ?? [])].join(', '),
        repository: automation.repoUrl
          ? repositoryLabel(automation.repoUrl)
          : '',
      })
    )
    .with({ kind: 'webhook' }, () => t('factoryRuns.automationSummaryWebhook'))
    .with({ kind: 'task_event' }, () =>
      t('factoryRuns.automationSummaryTaskEvent')
    )
    .exhaustive();
}

/** Says why a change was refused, with the service's message when it gave one. */
function refused(title: string, error: unknown) {
  if (error instanceof ThrownResultError) {
    toast.alert(title, { subtext: error.message });
    return;
  }
  toast.failure(title);
}

/**
 * The team's Automations: each starts a Run of a Flow as the person who
 * made it, on a schedule, a repository's GitHub event, a signed webhook call
 * or a change to a task. Members make their own and switch or delete them;
 * team admins change any.
 */
export function AutomationsSettings() {
  const query = useAutomationsQuery();
  const mutations = useAutomationMutations();
  const [creating, setCreating] = createSignal(false);
  const [secret, setSecret] = createSignal<CreatedAutomation>();

  const setEnabled = async (id: string, enabled: boolean) => {
    try {
      await mutations.setEnabled.mutateAsync({ id, enabled });
    } catch (error) {
      refused(t('factoryRuns.automationChangeFailed'), error);
    }
  };

  const remove = async (id: string) => {
    try {
      await mutations.remove.mutateAsync(id);
    } catch (error) {
      refused(t('factoryRuns.automationChangeFailed'), error);
    }
  };

  return (
    <Switch>
      <Match when={query.isPending}>
        <p class="p-3 text-sm text-ink-muted" role="status">
          {t('factoryRuns.automationsLoading')}
        </p>
      </Match>
      <Match when={query.isError}>
        <div class="flex items-center gap-2 p-3">
          <p class="text-sm text-ink-muted">
            {t('factoryRuns.automationsLoadFailed')}
          </p>
          <Button variant="ghost" size="xs" onClick={() => query.refetch()}>
            {t('common.retry')}
          </Button>
        </div>
      </Match>
      <Match when={query.data && !query.data.teamId}>
        <p class="p-3 text-sm text-ink-muted">
          {t('factoryRuns.automationsNoTeam')}
        </p>
      </Match>
      <Match when={query.data}>
        {(settings) => (
          <div class="flex flex-col gap-3 p-3">
            <Show
              when={settings().automations.length > 0}
              fallback={
                <p class="text-sm text-ink-muted">
                  {t('factoryRuns.automationsEmpty')}
                </p>
              }
            >
              <ul class="flex flex-col divide-y divide-edge-muted">
                <For each={settings().automations}>
                  {(automation) => (
                    <li class="flex flex-wrap items-center gap-x-3 gap-y-1 py-2">
                      <div class="flex min-w-0 flex-1 basis-60 flex-col gap-0.5">
                        <span class="text-sm font-medium [overflow-wrap:anywhere]">
                          {automation.name}
                        </span>
                        <span class="text-xs text-ink-muted [overflow-wrap:anywhere]">
                          {flowWordLabel(automation.flow)} ·{' '}
                          {triggerSummary(automation)}
                        </span>
                      </div>
                      <Checkbox
                        size="sm"
                        checked={automation.enabled}
                        onChange={(on: boolean) =>
                          fireAndForget(
                            setEnabled(automation.id, on),
                            'factory-runs'
                          )
                        }
                        disabled={mutations.setEnabled.isPending}
                      >
                        <Checkbox.Control />
                        <Checkbox.Label class="text-sm">
                          {t('factoryRuns.automationEnabled')}
                        </Checkbox.Label>
                      </Checkbox>
                      <Button
                        variant="danger-ghost"
                        size="xs"
                        disabled={mutations.remove.isPending}
                        onClick={() =>
                          fireAndForget(remove(automation.id), 'factory-runs')
                        }
                      >
                        {t('factoryRuns.deleteAutomation')}
                      </Button>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
            <Show when={secret()}>
              {(created) => (
                <WebhookSecret
                  created={created()}
                  onDone={() => setSecret(undefined)}
                />
              )}
            </Show>
            <p class="text-xs text-ink-muted">
              {t('factoryRuns.automationsHowTo')}{' '}
              {t('factoryRuns.automationsAdminsOnly')}
            </p>
            <Show
              when={creating()}
              fallback={
                <div class="flex justify-end">
                  <Button
                    variant="secondary"
                    size="sm"
                    onClick={() => setCreating(true)}
                  >
                    {t('factoryRuns.newAutomation')}
                  </Button>
                </div>
              }
            >
              <NewAutomationForm
                onCancel={() => setCreating(false)}
                onCreated={(created) => {
                  setCreating(false);
                  if (created.webhookSecret) setSecret(created);
                  toast.success(t('factoryRuns.automationCreated'));
                }}
              />
            </Show>
          </div>
        )}
      </Match>
    </Switch>
  );
}

/** A webhook's secret and address, shown once when it is made. */
function WebhookSecret(props: {
  created: CreatedAutomation;
  onDone: () => void;
}) {
  const url = () =>
    `${SERVER_HOSTS.factory}/automations/${props.created.automation.id}/webhook`;
  return (
    <div class="flex flex-col gap-1.5 rounded-md border border-edge-muted p-3">
      <p class="text-sm font-medium">
        {t('factoryRuns.automationSecretTitle')}
      </p>
      <p class="text-xs text-ink-muted">
        {t('factoryRuns.automationSecretHint')}
      </p>
      <code class="text-xs [overflow-wrap:anywhere]">{url()}</code>
      <code class="text-xs [overflow-wrap:anywhere]">
        {props.created.webhookSecret}
      </code>
      <div class="flex justify-end">
        <Button variant="ghost" size="xs" onClick={() => props.onDone()}>
          {t('common.dismiss')}
        </Button>
      </div>
    </div>
  );
}

/** A new Automation: its name, what starts it, its Flow and its request. */
function NewAutomationForm(props: {
  onCancel: () => void;
  onCreated: (created: CreatedAutomation) => void;
}) {
  const mutations = useAutomationMutations();
  const flows = useQuery(() => flowsOptions());
  const repositories = useAgentRepositoriesQuery();
  const [name, setName] = createSignal('');
  const [kind, setKind] = createSignal<TriggerKind>('schedule');
  const [cron, setCron] = createSignal('0 0 9 * * Mon-Fri');
  const [timezone, setTimezone] = createSignal(
    Intl.DateTimeFormat().resolvedOptions().timeZone
  );
  const [events, setEvents] = createSignal('');
  const [actions, setActions] = createSignal('');
  const [propertyId, setPropertyId] = createSignal<string>(
    SYSTEM_PROPERTY_IDS.STATUS
  );
  const [status, setStatus] = createSignal<string>(
    PROPERTY_OPTION_IDS.STATUS.IN_PROGRESS
  );
  const [values, setValues] = createSignal('');
  const onStatus = () => propertyId().trim() === SYSTEM_PROPERTY_IDS.STATUS;
  const [chosenFlow, setChosenFlow] = createSignal<string>();
  const [repoUrl, setRepoUrl] = createSignal<string>();
  const [request, setRequest] = createSignal('');

  const flow = () =>
    flows.data?.find((option) => option.name === chosenFlow()) ??
    flows.data?.[0];
  const repositoriesState = () => {
    if (repositories.isPending) return 'pending';
    if (repositories.isError) return 'error';
    return 'ready';
  };
  const needsRepository = () =>
    (flow()?.repository ?? false) || kind() === 'repository_event';

  const trigger = (): AutomationTrigger =>
    match(kind())
      .with('schedule', () => ({
        kind: 'schedule' as const,
        cron: cron().trim(),
        timezone: timezone().trim(),
      }))
      .with('repository_event', () => ({
        kind: 'repository_event' as const,
        events: wordsOf(events()),
        actions: wordsOf(actions()),
      }))
      .with('webhook', () => ({ kind: 'webhook' as const }))
      .with('task_event', () => ({
        kind: 'task_event' as const,
        propertyId: propertyId().trim(),
        values: onStatus() ? [status()] : wordsOf(values()),
      }))
      .exhaustive();

  const canSubmit = () =>
    !mutations.create.isPending &&
    name().trim().length > 0 &&
    request().trim().length > 0 &&
    flow() !== undefined &&
    (!needsRepository() || repoUrl() !== undefined);

  const submit = async (event: SubmitEvent) => {
    event.preventDefault();
    const chosen = flow();
    if (!canSubmit() || !chosen) return;
    try {
      const created = await mutations.create.mutateAsync({
        name: name().trim(),
        trigger: trigger(),
        flow: chosen.name,
        repoUrl: needsRepository() ? repoUrl() : undefined,
        requestTemplate: request().trim(),
      });
      props.onCreated(created);
    } catch (error) {
      refused(t('factoryRuns.automationCreateFailed'), error);
    }
  };

  return (
    <form
      class="flex flex-col gap-3 border-t border-edge-muted pt-3"
      aria-busy={mutations.create.isPending}
      onSubmit={(event) => fireAndForget(submit(event), 'factory-runs')}
    >
      <TextField value={name()} onChange={setName}>
        <TextField.Label class="text-sm">
          {t('factoryRuns.automationName')}
        </TextField.Label>
        <TextField.Input />
      </TextField>
      <div class="flex flex-col gap-2">
        <span class="text-sm">{t('factoryRuns.automationTrigger')}</span>
        <Tabs
          touch="segmented"
          size="sm"
          aria-label={t('factoryRuns.automationTrigger')}
          value={kind()}
          options={TRIGGER_KINDS.map((option) => ({
            value: option.value,
            label: t(option.label),
          }))}
          onChange={setKind}
        />
      </div>
      <Switch>
        <Match when={kind() === 'schedule'}>
          <TextField value={cron()} onChange={setCron}>
            <TextField.Label class="text-sm">
              {t('factoryRuns.automationCron')}
            </TextField.Label>
            <TextField.Input class="font-mono" />
            <TextField.Description class="text-xs text-ink-muted">
              {t('factoryRuns.automationCronHint')}
            </TextField.Description>
          </TextField>
          <TextField value={timezone()} onChange={setTimezone}>
            <TextField.Label class="text-sm">
              {t('factoryRuns.automationTimezone')}
            </TextField.Label>
            <TextField.Input />
          </TextField>
        </Match>
        <Match when={kind() === 'repository_event'}>
          <TextField value={events()} onChange={setEvents}>
            <TextField.Label class="text-sm">
              {t('factoryRuns.automationEvents')}
            </TextField.Label>
            <TextField.Input
              class="font-mono"
              placeholder={t('factoryRuns.automationEventsPlaceholder')}
            />
          </TextField>
          <TextField value={actions()} onChange={setActions}>
            <TextField.Label class="text-sm">
              {t('factoryRuns.automationActions')}
            </TextField.Label>
            <TextField.Input
              class="font-mono"
              placeholder={t('factoryRuns.automationActionsPlaceholder')}
            />
          </TextField>
        </Match>
        <Match when={kind() === 'task_event'}>
          <TextField value={propertyId()} onChange={setPropertyId}>
            <TextField.Label class="text-sm">
              {t('factoryRuns.automationProperty')}
            </TextField.Label>
            <TextField.Input class="font-mono" />
            <TextField.Description class="text-xs text-ink-muted">
              {t('factoryRuns.automationPropertyHint')}
            </TextField.Description>
          </TextField>
          <Show
            when={onStatus()}
            fallback={
              <TextField value={values()} onChange={setValues}>
                <TextField.Label class="text-sm">
                  {t('factoryRuns.automationValues')}
                </TextField.Label>
                <TextField.Input class="font-mono" />
              </TextField>
            }
          >
            <div class="flex flex-col gap-2">
              <span class="text-sm">{t('factoryRuns.automationStatus')}</span>
              <Tabs
                touch="segmented"
                size="sm"
                aria-label={t('factoryRuns.automationStatus')}
                value={status()}
                options={STATUSES.map((option) => ({
                  value: option.value,
                  label: t(option.label),
                }))}
                onChange={setStatus}
              />
            </div>
          </Show>
        </Match>
        <Match when={kind() === 'webhook'}>
          <p class="text-xs text-ink-muted">
            {t('factoryRuns.automationWebhookHint')}
          </p>
        </Match>
      </Switch>
      <Show when={flows.data}>
        {(options) => (
          <RadioGroup
            variant="cards"
            value={flow()?.name}
            onChange={setChosenFlow}
            class="grid-cols-1"
          >
            <RadioGroup.Label class="text-sm">
              {t('factoryRuns.flowLabel')}
            </RadioGroup.Label>
            <For each={options()}>
              {(option) => (
                <RadioGroup.Item value={option.name}>
                  <div class="flex min-w-0 flex-1 flex-col gap-0.5">
                    <RadioGroup.ItemLabel class="text-sm font-medium text-ink">
                      {flowWordLabel(option.name)}
                    </RadioGroup.ItemLabel>
                    <RadioGroup.ItemDescription class="text-xs text-ink-muted">
                      {option.description}
                    </RadioGroup.ItemDescription>
                  </div>
                  <RadioGroup.ItemControl class="mt-0.5" />
                </RadioGroup.Item>
              )}
            </For>
          </RadioGroup>
        )}
      </Show>
      <Show when={needsRepository()}>
        <RepositoryField
          repositories={repositories.data?.repositories.map(
            (repository) => repository.url
          )}
          state={repositoriesState()}
          onRetry={() => repositories.refetch()}
          value={repoUrl()}
          onChange={setRepoUrl}
        />
      </Show>
      <TextField value={request()} onChange={setRequest}>
        <TextField.Label class="text-sm">
          {t('factoryRuns.automationRequest')}
        </TextField.Label>
        <TextField.TextArea autoResize rows={3} />
      </TextField>
      <div class="flex justify-end gap-2">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          disabled={mutations.create.isPending}
          onClick={() => props.onCancel()}
        >
          {t('common.cancel')}
        </Button>
        <Button
          type="submit"
          variant="primary"
          size="sm"
          loading={mutations.create.isPending}
          disabled={!mutations.create.isPending && !canSubmit()}
        >
          {t('factoryRuns.createAutomation')}
        </Button>
      </div>
    </form>
  );
}
