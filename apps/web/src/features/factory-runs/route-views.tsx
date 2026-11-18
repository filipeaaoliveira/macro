import { useRouteParams } from '@app/lib/split-router';
import { factoryRunRoute } from '@app/routes/routes';
import { withAuth } from '@components/app/split-layout/split-router/app-route-shell';
import { FactoryInsights } from './factory-runs';

export const FactoryInsightsRouteView = withAuth(() => <FactoryInsights />);

/** A Run's link, as a notification opens it: the Run over the insights. */
export const FactoryRunRouteView = withAuth(() => {
  const params = useRouteParams(factoryRunRoute);
  return <FactoryInsights openRunId={params.runId} />;
});
