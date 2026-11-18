import { t } from '@app/lib/i18n';
import { Button } from '@ui';
import { type Accessor, Match, Switch } from 'solid-js';
import { EvaluationSections } from '../../components/evaluation-sections';
import { useInsightsContext } from '../../context/insights-context';
import type { InsightsParams } from '../../core/insights';

/**
 * The Scorers, Experiments and Benchmarks of the insights' period, read
 * apart from the Run insights so either shows while the other loads.
 */
export function EvaluationPanel(props: {
  params: Accessor<InsightsParams>;
  onOpenRun: (runId: string) => void;
}) {
  const source = useInsightsContext().evaluation(props.params);

  return (
    <Switch>
      <Match when={source.state() === 'error'}>
        <div
          role="alert"
          class="mt-5 flex flex-wrap items-center justify-between gap-4 rounded-xl border border-edge-muted p-5"
        >
          <p class="text-sm">{t('factoryEvaluation.loadFailed')}</p>
          <Button variant="secondary" onClick={source.retry}>
            {t('common.retry')}
          </Button>
        </div>
      </Match>
      <Match when={source.data()}>
        {(evaluation) => (
          <div class="mt-5" classList={{ 'opacity-50': source.updating() }}>
            <EvaluationSections
              evaluation={evaluation()}
              onOpenRun={props.onOpenRun}
            />
          </div>
        )}
      </Match>
    </Switch>
  );
}
