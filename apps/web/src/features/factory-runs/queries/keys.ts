import { createQueryKeys } from '@lukemorales/query-key-factory';
import type { InsightsParams } from '../core/insights';

export const factoryRunKeys = createQueryKeys('factoryRuns', {
  task: (taskId: string) => ({ queryKey: [taskId] }),
  run: (runId: string) => ({ queryKey: [runId] }),
  waiting: null,
  flows: null,
  controlFiles: null,
  standingRules: null,
  insights: (params: InsightsParams) => ({
    queryKey: [params.from, params.to, params.timeZone, params.flow ?? ''],
  }),
});
