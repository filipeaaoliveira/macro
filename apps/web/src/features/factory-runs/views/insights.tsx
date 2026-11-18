import { ListHeader } from '@app/components/view-shell/ListHeader';
import { ViewSidebar } from '@app/components/view-shell/ViewSidebar';
import { t } from '@app/lib/i18n';
import { HeaderActionGroup } from '@components/app/HeaderActionGroup';
import type { RunsSource } from '../context/runs-context';
import type { RunInsights } from '../core/insights';
import type { InsightsState } from '../primitives/insights-state';
import { InsightsBody } from './shared/insights-body';
import { InsightsFilters } from './shared/insights-filters';
import { WaitingRuns } from './shared/waiting-runs';

/** Run insights on desktop: a list header with the filters, then the page. */
export function InsightsView(props: {
  state: InsightsState;
  source: RunsSource<RunInsights> & { updating: () => boolean };
  flows: string[];
}) {
  return (
    <div class="flex size-full min-h-0 flex-col">
      <ListHeader
        title={
          <ViewSidebar.Title caret={false}>
            {t('factoryInsights.title')}
          </ViewSidebar.Title>
        }
      >
        <HeaderActionGroup label={t('factoryInsights.actions')} size="md">
          <InsightsFilters state={props.state} flows={props.flows} grouped />
        </HeaderActionGroup>
      </ListHeader>
      <div class="min-h-0 flex-1 overflow-y-auto px-4 pb-8 pt-3">
        <WaitingRuns onOpenRun={props.state.setOpenRunId} />
        <InsightsBody
          source={props.source}
          onOpenRun={props.state.setOpenRunId}
        />
      </div>
    </div>
  );
}
