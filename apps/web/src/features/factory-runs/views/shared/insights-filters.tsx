import { SelectInput } from '@app/features/scheduling/components/fields';
import { SchedulingFilterSection } from '@app/features/scheduling/components/scheduling-filter-section';
import { SchedulingFilters } from '@app/features/scheduling/components/scheduling-filters';
import { t } from '@app/lib/i18n';
import { flowWordLabel } from '../../core/flow-words';
import { INSIGHT_PERIODS } from '../../core/insights';
import type { InsightsState } from '../../primitives/insights-state';

const ALL_FLOWS = 'all';

/**
 * The Flow and the period, as Scheduling's insights filter them: a popover
 * of selects on desktop, a drawer of options on touch.
 */
export function InsightsFilters(props: {
  state: InsightsState;
  flows: string[];
  grouped?: boolean;
}) {
  const flowOptions = () => [
    { value: ALL_FLOWS, label: t('factoryInsights.allFlows') },
    ...props.flows.map((flow) => ({ value: flow, label: flowWordLabel(flow) })),
  ];
  const periodOptions = () =>
    INSIGHT_PERIODS.map((days) => ({
      value: String(days),
      label: t('factoryInsights.lastDays', { count: days }),
    }));
  const flow = () => props.state.flow() ?? ALL_FLOWS;
  const setFlow = (value: string) =>
    props.state.setFlow(value === ALL_FLOWS ? undefined : value);
  const setPeriod = (value: string) => {
    const days = INSIGHT_PERIODS.find((period) => String(period) === value);
    if (days) props.state.setPeriod(days);
  };

  return (
    <SchedulingFilters
      grouped={props.grouped}
      size="md"
      title={t('factoryInsights.filters')}
      activeCount={
        Number(props.state.flow() !== undefined) +
        Number(props.state.period() !== 30)
      }
      onClear={props.state.clearFilters}
      mobileChildren={
        <>
          <SchedulingFilterSection
            value="flow"
            label={t('factoryInsights.flow')}
            selected={flow()}
            options={flowOptions()}
            onChange={setFlow}
          />
          <SchedulingFilterSection
            value="period"
            label={t('factoryInsights.period')}
            selected={String(props.state.period())}
            activeCount={Number(props.state.period() !== 30)}
            options={periodOptions()}
            onChange={setPeriod}
          />
        </>
      }
    >
      <SelectInput
        label={t('factoryInsights.flow')}
        class="w-full text-sm"
        value={flow()}
        options={flowOptions()}
        onChange={setFlow}
      />
      <SelectInput
        label={t('factoryInsights.period')}
        class="w-full text-sm"
        value={String(props.state.period())}
        options={periodOptions()}
        onChange={setPeriod}
      />
    </SchedulingFilters>
  );
}
