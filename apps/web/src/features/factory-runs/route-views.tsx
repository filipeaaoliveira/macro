import { useRouteParams } from '@app/lib/split-router';
import { factoryRunRoute } from '@app/routes/routes';
import { withAuth } from '@components/app/split-layout/split-router/app-route-shell';
import { FactoryBoard, FactoryInsights, FactoryRun } from './factory-runs';

export const FactoryInsightsRouteView = withAuth(() => <FactoryInsights />);

/** A Run's link, as the board and a notification open it: the Run as a pane. */
export const FactoryRunRouteView = withAuth(() => {
  const params = useRouteParams(factoryRunRoute);
  return <FactoryRun runId={params.runId} />;
});

/** The factory's board: every Run the person can see. */
export const FactoryBoardRouteView = withAuth(() => <FactoryBoard />);
