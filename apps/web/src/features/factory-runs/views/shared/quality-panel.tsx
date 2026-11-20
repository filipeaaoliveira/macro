import { ListSkeleton } from '@app/components/view-shell/ListSkeleton';
import { SelectInput } from '@app/features/scheduling/components/fields';
import { t } from '@app/lib/i18n';
import { Button } from '@ui';
import {
  type Accessor,
  createMemo,
  createSignal,
  Index,
  Match,
  Show,
  Switch,
} from 'solid-js';
import { QualitySections } from '../../components/quality-sections';
import { useInsightsContext } from '../../context/insights-context';
import type { InsightsParams } from '../../core/insights';
import { type QualityParams, repositoryName } from '../../core/quality';

const ALL_REPOSITORIES = 'all';

/**
 * CI health, review quality, cost and autonomy of the insights' period,
 * read apart from the Run insights so either shows while the other loads,
 * for one repository picked here or for all.
 */
export function QualityPanel(props: {
  params: Accessor<InsightsParams>;
  onOpenRun: (runId: string) => void;
}) {
  const [repository, setRepository] = createSignal<string>();
  const params = createMemo<QualityParams>(() => ({
    ...props.params(),
    repository: repository(),
  }));
  const source = useInsightsContext().quality(params);
  // The repositories to pick from: the period's, and the one picked.
  const options = () => {
    const urls = new Set(source.data()?.repositories ?? []);
    const picked = repository();
    if (picked) urls.add(picked);
    return [
      { value: ALL_REPOSITORIES, label: t('factoryQuality.allRepositories') },
      ...[...urls]
        .sort()
        .map((url) => ({ value: url, label: repositoryName(url) })),
    ];
  };

  return (
    <div class="mt-5 flex min-w-0 flex-col gap-3">
      <Show when={options().length > 1}>
        <div class="flex justify-end">
          <SelectInput
            label={t('factoryQuality.repository')}
            class="w-full max-w-64 text-sm"
            value={repository() ?? ALL_REPOSITORIES}
            options={options()}
            onChange={(value) =>
              setRepository(value === ALL_REPOSITORIES ? undefined : value)
            }
          />
        </div>
      </Show>
      <Switch>
        <Match when={source.state() === 'pending'}>
          <ListSkeleton.Root label={t('factoryQuality.loading')}>
            <div class="flex flex-col gap-3 border-t border-edge-muted py-5">
              <ListSkeleton.Bar class="h-4 w-24" />
              <Index each={[0, 1, 2]}>
                {() => <ListSkeleton.Bar class="w-full max-w-96" />}
              </Index>
            </div>
          </ListSkeleton.Root>
        </Match>
        <Match when={source.state() === 'error'}>
          <div
            role="alert"
            class="flex flex-wrap items-center justify-between gap-4 rounded-xl border border-edge-muted p-5"
          >
            <p class="text-sm">{t('factoryQuality.loadFailed')}</p>
            <Button variant="secondary" onClick={source.retry}>
              {t('common.retry')}
            </Button>
          </div>
        </Match>
        <Match when={source.data()}>
          {(quality) => (
            <div classList={{ 'opacity-50': source.updating() }}>
              <QualitySections
                quality={quality()}
                onOpenRun={props.onOpenRun}
              />
            </div>
          )}
        </Match>
      </Switch>
    </div>
  );
}
