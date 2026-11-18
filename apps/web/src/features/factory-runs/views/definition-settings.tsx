import { t } from '@app/lib/i18n';
import { toast } from '@core/component/Toast/Toast';
import { fireAndForget } from '@core/util/fireAndForget';
import { ThrownResultError } from '@core/util/result';
import { Button, TextField } from '@ui';
import { createSignal, Match, Show, Switch } from 'solid-js';
import {
  useDefinitionQuery,
  useSetDefinitionMutation,
} from '../queries/definition';

/**
 * The team's Factory Definition: the repository whose files its Runs take
 * their Flows, Agents and rules from, and whether it reads at its head. Its
 * admins name one or stop using it; other members read it.
 */
export function DefinitionSettings() {
  const query = useDefinitionQuery();
  const mutation = useSetDefinitionMutation();
  const [draft, setDraft] = createSignal<string>();

  const save = async (repoUrl: string | null) => {
    try {
      await mutation.mutateAsync(repoUrl);
      setDraft(undefined);
      toast.success(t('factoryRuns.definitionSaved'));
    } catch (error) {
      // A refusal says why: a Definition that doesn't read, a repository
      // out of reach, or a team the person no longer administers.
      if (error instanceof ThrownResultError) {
        toast.alert(t('factoryRuns.definitionSaveFailed'), {
          subtext: error.message,
        });
        return;
      }
      toast.failure(t('factoryRuns.definitionSaveFailed'));
    }
  };

  return (
    <Switch>
      <Match when={query.isPending}>
        <p class="p-3 text-sm text-ink-muted" role="status">
          {t('factoryRuns.definitionLoading')}
        </p>
      </Match>
      <Match when={query.isError}>
        <div class="flex items-center gap-2 p-3">
          <p class="text-sm text-ink-muted">
            {t('factoryRuns.definitionLoadFailed')}
          </p>
          <Button variant="ghost" size="xs" onClick={() => query.refetch()}>
            {t('common.retry')}
          </Button>
        </div>
      </Match>
      <Match when={query.data}>
        {(settings) => {
          const text = () => draft() ?? settings().repoUrl ?? '';
          const locked = () => !settings().canEdit || mutation.isPending;
          return (
            <div class="flex flex-col gap-2 p-3">
              <TextField value={text()} onChange={setDraft} disabled={locked()}>
                <TextField.Label class="text-sm">
                  {t('factoryRuns.definitionRepository')}
                </TextField.Label>
                <TextField.Input
                  class="font-mono"
                  placeholder={t('factoryRuns.definitionPlaceholder')}
                />
              </TextField>
              <Show
                when={settings().repoUrl}
                fallback={
                  <p class="text-xs text-ink-muted">
                    {t('factoryRuns.definitionNone')}
                  </p>
                }
              >
                <Show when={settings().commit}>
                  {(commit) => (
                    <p class="text-xs text-ink-muted">
                      {t('factoryRuns.definitionReads', {
                        commit: commit().slice(0, 7),
                      })}
                    </p>
                  )}
                </Show>
                <Show when={settings().problem}>
                  {(problem) => (
                    <p class="text-xs text-failure" role="alert">
                      {t('factoryRuns.definitionProblem', {
                        problem: problem(),
                      })}
                    </p>
                  )}
                </Show>
                <Show when={settings().requiresPullRequest === true}>
                  <p class="text-xs text-ink-muted">
                    {t('factoryRuns.definitionPullRequestRequired')}
                  </p>
                </Show>
                <Show when={settings().requiresPullRequest === false}>
                  <p class="text-xs text-failure" role="alert">
                    {t('factoryRuns.definitionNoPullRequest')}
                  </p>
                </Show>
                <Show
                  when={
                    settings().commit && settings().requiresPullRequest === null
                  }
                >
                  <p class="text-xs text-ink-muted">
                    {t('factoryRuns.definitionPullRequestUnknown')}
                  </p>
                </Show>
                <p class="text-xs text-ink-muted">
                  {t('factoryRuns.definitionHowTo')}
                </p>
              </Show>
              <Show
                when={settings().canEdit}
                fallback={
                  <p class="text-xs text-ink-muted">
                    {t('factoryRuns.definitionAdminsOnly')}
                  </p>
                }
              >
                <div class="flex justify-end gap-2">
                  <Button
                    variant="secondary"
                    size="sm"
                    disabled={locked() || !settings().repoUrl}
                    onClick={() => fireAndForget(save(null), 'factory-runs')}
                  >
                    {t('factoryRuns.definitionRemove')}
                  </Button>
                  <Button
                    variant="primary"
                    size="sm"
                    disabled={
                      locked() ||
                      draft() === undefined ||
                      text().trim().length === 0
                    }
                    onClick={() =>
                      fireAndForget(save(text().trim()), 'factory-runs')
                    }
                  >
                    {t('factoryRuns.definitionSave')}
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
