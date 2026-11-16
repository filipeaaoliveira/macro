import { withAuth } from '@components/app/split-layout/split-router/app-route-shell';
import { FactoryInsights } from './factory-runs';

export const FactoryInsightsRouteView = withAuth(() => <FactoryInsights />);
