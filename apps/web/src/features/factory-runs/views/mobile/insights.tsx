import { t } from '@app/lib/i18n';
import { MobileHeader } from '@components/app/mobile-header/mobile-header';
import type { RunsSource } from '../../context/runs-context';
import type { RunInsights } from '../../core/insights';
import type { InsightsState } from '../../primitives/insights-state';
import { EvaluationPanel } from '../shared/evaluation-panel';
import { InsightsBody } from '../shared/insights-body';
import { InsightsFilters } from '../shared/insights-filters';
import { WaitingRuns } from '../shared/waiting-runs';

/**
 * Run insights on a phone or tablet: the title in the header, the filters
 * above the period, and the sections in one column.
 */
export function MobileInsightsView(props: {
  state: InsightsState;
  source: RunsSource<RunInsights> & { updating: () => boolean };
  flows: string[];
}) {
  return (
    <>
      <MobileHeader>
        <MobileHeader.Title label={t('factoryInsights.title')} />
      </MobileHeader>
      <div class="size-full min-h-0 overflow-y-auto px-4 pb-(--mobile-content-inset-bottom) pt-(--mobile-content-inset-top)">
        <WaitingRuns onOpenRun={props.state.setOpenRunId} />
        <InsightsBody
          source={props.source}
          onOpenRun={props.state.setOpenRunId}
          toolbar={
            <div class="flex justify-end">
              <InsightsFilters state={props.state} flows={props.flows} />
            </div>
          }
        />
        <EvaluationPanel
          params={props.state.params}
          onOpenRun={props.state.setOpenRunId}
        />
      </div>
    </>
  );
}
