import { t } from '@app/lib/i18n';
import { MobileHeader } from '@components/app/mobile-header/mobile-header';
import type { RunsSource } from '../../context/runs-context';
import type { RunInsights } from '../../core/insights';
import type { InsightsState } from '../../primitives/insights-state';
import { InsightsBody } from '../shared/insights-body';
import { InsightsFilters } from '../shared/insights-filters';

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
        <InsightsBody
          source={props.source}
          onOpenRun={props.state.setOpenRunId}
          toolbar={
            <div class="flex justify-end">
              <InsightsFilters state={props.state} flows={props.flows} />
            </div>
          }
        />
      </div>
    </>
  );
}
