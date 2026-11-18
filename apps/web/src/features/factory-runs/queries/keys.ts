import { createQueryKeys } from '@lukemorales/query-key-factory';
import type { BoardFilters } from '../core/board';
import type { InsightsParams } from '../core/insights';

export const factoryRunKeys = createQueryKeys('factoryRuns', {
  task: (taskId: string) => ({ queryKey: [taskId] }),
  run: (runId: string) => ({ queryKey: [runId] }),
  waiting: null,
  flows: null,
  controlFiles: null,
  standingRules: null,
  automations: null,
  sessionStep: (sessionId: string) => ({ queryKey: [sessionId] }),
  board: (filters: BoardFilters) => ({
    queryKey: [
      filters.scope,
      filters.state ?? '',
      filters.flow ?? '',
      filters.repoUrl ?? '',
      filters.projectId ?? '',
      filters.search ?? '',
    ],
  }),
  insights: (params: InsightsParams) => ({
    queryKey: [params.from, params.to, params.timeZone, params.flow ?? ''],
  }),
});
