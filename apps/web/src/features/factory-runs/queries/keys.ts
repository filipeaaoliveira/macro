import { createQueryKeys } from '@lukemorales/query-key-factory';

export const factoryRunKeys = createQueryKeys('factoryRuns', {
  task: (taskId: string) => ({ queryKey: [taskId] }),
  run: (runId: string) => ({ queryKey: [runId] }),
  flows: null,
});
