import { ListHeader } from '@app/components/view-shell/ListHeader';
import { ViewSidebar } from '@app/components/view-shell/ViewSidebar';
import { t } from '@app/lib/i18n';
import { HeaderActionGroup } from '@components/app/HeaderActionGroup';
import CaretDownIcon from '@icon/system/regular/caret-down.svg';
import { Button, cn, Tabs } from '@ui';
import { createSignal } from 'solid-js';
import type { RunsSource } from '../context/runs-context';
import { INSIGHT_PERIODS, type RunInsights } from '../core/insights';
import type { InsightsState } from '../primitives/insights-state';
import { EvaluationPanel } from './shared/evaluation-panel';
import { InsightsBody } from './shared/insights-body';
import { InsightsFilters } from './shared/insights-filters';
import { InsightsOverviewPanel } from './shared/insights-overview-panel';
import { QualityPanel } from './shared/quality-panel';
import { WaitingRuns } from './shared/waiting-runs';

/** Run insights on desktop: a list header with the filters, then the page. */
export function InsightsView(props: {
  state: InsightsState;
  source: RunsSource<RunInsights> & { updating: () => boolean };
  flows: string[];
}) {
  const [details, setDetails] = createSignal(false);
  return (
    <div class="flex size-full min-h-0 flex-col">
      <ListHeader
        title={
          <ViewSidebar.Title caret={false}>
            {t('factoryInsights.title')}
          </ViewSidebar.Title>
        }
      >
        <Tabs
          aria-label={t('factoryInsights.period')}
          size="md"
          options={INSIGHT_PERIODS.map((days) => ({
            value: String(days),
            label: t('factoryInsights.periodDays', { count: days }),
          }))}
          value={String(props.state.period())}
          onChange={(value: string) =>
            props.state.setPeriod(
              INSIGHT_PERIODS.find((days) => String(days) === value) ?? 30
            )
          }
        />
        <HeaderActionGroup label={t('factoryInsights.actions')} size="md">
          <InsightsFilters state={props.state} flows={props.flows} grouped />
        </HeaderActionGroup>
      </ListHeader>
      <div class="mx-auto min-h-0 w-full flex-1 overflow-y-auto px-4 pb-8 pt-3 sm:px-6">
        <WaitingRuns onOpenRun={props.state.setOpenRunId} />
        <InsightsOverviewPanel
          params={props.state.params}
          source={props.source}
        />
        <div class="flex justify-center border-t border-edge-divider pt-5">
          <Button
            variant="secondary"
            size="sm"
            aria-expanded={details()}
            onClick={() => setDetails(!details())}
          >
            <CaretDownIcon
              class={cn(
                'size-3.5 transition-transform duration-150',
                details() && 'rotate-180'
              )}
            />
            {details()
              ? t('factoryInsights.hideStepDetails')
              : t('factoryInsights.showStepDetails')}
          </Button>
        </div>
        {/* Closed, the details stay mounted, out of sight and reach. */}
        <div
          inert={!details()}
          class={cn('pt-5', !details() && 'h-0 overflow-hidden pt-0')}
        >
          <InsightsBody
            source={props.source}
            onOpenRun={props.state.setOpenRunId}
          />
          <QualityPanel
            params={props.state.params}
            onOpenRun={props.state.setOpenRunId}
          />
          <EvaluationPanel
            params={props.state.params}
            onOpenRun={props.state.setOpenRunId}
          />
        </div>
      </div>
    </div>
  );
}
