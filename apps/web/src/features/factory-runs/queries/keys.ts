import { createQueryKeys } from '@lukemorales/query-key-factory';
import type { BoardFilters } from '../core/board';
import type { InsightsParams } from '../core/insights';
import type { QualityParams } from '../core/quality';

export const factoryRunKeys = createQueryKeys('factoryRuns', {
  task: (taskId: string) => ({ queryKey: [taskId] }),
  run: (runId: string) => ({ queryKey: [runId] }),
  runScores: (runId: string) => ({ queryKey: [runId] }),
  waiting: null,
  flows: null,
  controlFiles: null,
  definition: null,
  definitionFile: (path: string) => ({ queryKey: [path] }),
  standingRules: null,
  automations: null,
  sessionStep: (sessionId: string) => ({ queryKey: [sessionId] }),
  pullRequests: (urls: readonly string[]) => ({
    queryKey: [urls.join('\n')],
  }),
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
  evaluation: (params: InsightsParams) => ({
    queryKey: [params.from, params.to, params.timeZone, params.flow ?? ''],
  }),
  quality: (params: QualityParams) => ({
    queryKey: [
      params.from,
      params.to,
      params.timeZone,
      params.flow ?? '',
      params.repository ?? '',
    ],
  }),
});
