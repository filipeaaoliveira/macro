import { isTouchDevice } from '@core/mobile/isTouchDevice';
import { createEffect, on, Show } from 'solid-js';
import { useInsightsContext } from './context/insights-context';
import { createInsightsState } from './primitives/insights-state';
import { RunView } from './run-view';
import { InsightsView } from './views/insights';
import { MobileInsightsView } from './views/mobile/insights';

/**
 * Run insights: its state sits here, above the one device switch, so the
 * desktop and phone views share it. A Run opened from it opens as from a
 * task.
 */
export function RunInsightsView(props: {
  timeZone: string;
  now: () => Date;
  /** A Run to open at once, as a link to it does. */
  openRunId?: string;
}) {
  const context = useInsightsContext();
  const state = createInsightsState(props.timeZone, props.now);
  // A link opens its Run, and a pane reused for another Run's link opens
  // that one.
  createEffect(
    on(
      () => props.openRunId,
      (runId) => {
        if (runId) state.setOpenRunId(runId);
      }
    )
  );
  const source = context.insights(state.params);
  const flowNames = context.flowNames();
  // The filter offers the Flows a Run can start from, and any Flow the
  // period's Runs walk that is no longer offered.
  const flows = () => {
    const names = new Set(flowNames.data() ?? []);
    for (const flow of source.data()?.flows ?? []) {
      if (flow.flow) names.add(flow.flow);
    }
    const selected = state.flow();
    if (selected) names.add(selected);
    return [...names].sort();
  };

  return (
    <>
      <Show
        when={isTouchDevice()}
        fallback={
          <InsightsView state={state} source={source} flows={flows()} />
        }
      >
        <MobileInsightsView state={state} source={source} flows={flows()} />
      </Show>
      <Show when={state.openRunId()} keyed>
        {(runId) => (
          <RunView
            runId={runId}
            onClose={() => state.setOpenRunId(undefined)}
          />
        )}
      </Show>
    </>
  );
}
