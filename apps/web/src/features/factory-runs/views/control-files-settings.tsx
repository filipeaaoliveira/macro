import { t } from '@app/lib/i18n';
import { toast } from '@core/component/Toast/Toast';
import { fireAndForget } from '@core/util/fireAndForget';
import { Button, TextField } from '@ui';
import { createSignal, Match, Switch } from 'solid-js';
import {
  useControlFilesQuery,
  useSetControlFilesMutation,
} from '../queries/control-files';

/** The patterns in `text`, one a line, blank lines left out. */
function patternsOf(text: string): string[] {
  return text
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

/**
 * The team's Control Files: the patterns its Runs' changes are checked
 * against, one a line. Its admins edit them or go back to the default;
 * other members read them.
 */
export function ControlFilesSettings() {
  const query = useControlFilesQuery();
  const mutation = useSetControlFilesMutation();
  const [draft, setDraft] = createSignal<string>();

  const save = async (patterns: string[] | null) => {
    try {
      await mutation.mutateAsync(patterns);
      setDraft(undefined);
      toast.success(t('factoryRuns.controlFilesSaved'));
    } catch {
      toast.failure(t('factoryRuns.controlFilesSaveFailed'));
    }
  };

  return (
    <Switch>
      <Match when={query.isPending}>
        <p class="p-3 text-sm text-ink-muted" role="status">
          {t('factoryRuns.controlFilesLoading')}
        </p>
      </Match>
      <Match when={query.isError}>
        <div class="flex items-center gap-2 p-3">
          <p class="text-sm text-ink-muted">
            {t('factoryRuns.controlFilesLoadFailed')}
          </p>
          <Button variant="ghost" size="xs" onClick={() => query.refetch()}>
            {t('common.retry')}
          </Button>
        </div>
      </Match>
      <Match when={query.data}>
        {(settings) => {
          const text = () => draft() ?? settings().patterns.join('\n');
          const locked = () => !settings().canEdit || mutation.isPending;
          return (
            <div class="flex flex-col gap-2 p-3">
              <TextField value={text()} onChange={setDraft} disabled={locked()}>
                <TextField.Label class="text-sm">
                  {t('factoryRuns.controlFilesPatterns')}
                </TextField.Label>
                <TextField.TextArea
                  rows={8}
                  class="max-h-64 overflow-y-auto font-mono text-xs"
                />
              </TextField>
              <p class="text-xs text-ink-muted">
                {settings().isDefault
                  ? t('factoryRuns.controlFilesDefault')
                  : t('factoryRuns.controlFilesOwn')}{' '}
                {settings().canEdit
                  ? t('factoryRuns.controlFilesHowTo')
                  : t('factoryRuns.controlFilesAdminsOnly')}
              </p>
              <Switch>
                <Match when={settings().canEdit}>
                  <div class="flex justify-end gap-2">
                    <Button
                      variant="secondary"
                      size="sm"
                      disabled={locked() || settings().isDefault}
                      onClick={() => fireAndForget(save(null), 'factory-runs')}
                    >
                      {t('factoryRuns.controlFilesUseDefault')}
                    </Button>
                    <Button
                      variant="primary"
                      size="sm"
                      disabled={locked() || draft() === undefined}
                      onClick={() =>
                        fireAndForget(save(patternsOf(text())), 'factory-runs')
                      }
                    >
                      {t('factoryRuns.controlFilesSave')}
                    </Button>
                  </div>
                </Match>
              </Switch>
            </div>
          );
        }}
      </Match>
    </Switch>
  );
}
