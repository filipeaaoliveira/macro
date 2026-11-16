import { SERVER_HOSTS } from '@core/constant/servers';
import { fetchWithToken } from '@core/util/fetchWithToken';
import type { RunDetail } from './generated/schemas';
import type {
  FlowsResponse,
  GateAnswerRequest,
  StartRunRequest,
  TaskRunsResponse,
} from './pending-routes';

export type {
  FlowSummary,
  GateAnswerRequest,
  StartRunRequest,
} from './pending-routes';

const host = () => SERVER_HOSTS.factory;

/** The software factory's Runs, as the signed-in person sees them. */
export const factoryClient = {
  /** The Runs of a task, newest first. */
  listTaskRuns(taskId: string, signal?: AbortSignal) {
    return fetchWithToken<TaskRunsResponse>(
      `${host()}/tasks/${encodeURIComponent(taskId)}/runs`,
      { method: 'GET', signal }
    ).then((result) => result.map((data) => data.runs));
  },

  getRun(runId: string, signal?: AbortSignal) {
    return fetchWithToken<RunDetail>(
      `${host()}/runs/${encodeURIComponent(runId)}`,
      {
        method: 'GET',
        signal,
      }
    );
  },

  /** The Flows a Run can be started from. */
  listFlows(signal?: AbortSignal) {
    return fetchWithToken<FlowsResponse>(`${host()}/flows`, {
      method: 'GET',
      signal,
    }).then((result) => result.map((data) => data.flows));
  },

  startRun(request: StartRunRequest) {
    return fetchWithToken<RunDetail>(`${host()}/runs`, {
      method: 'POST',
      body: JSON.stringify(request),
    });
  },

  /**
   * Answers the Gate the Run waits on at `entry`'s `visit`. A Gate that was
   * already answered, or that the Run no longer waits on, fails `CONFLICT`.
   */
  answerGate(
    runId: string,
    entry: string,
    visit: number,
    answer: GateAnswerRequest
  ) {
    return fetchWithToken<Record<string, never>>(
      `${host()}/runs/${encodeURIComponent(runId)}/gates/${encodeURIComponent(entry)}/${visit}/answer`,
      { method: 'POST', body: JSON.stringify(answer) }
    ).then((result) => result.map(() => undefined));
  },

  /** Cancels a Run. One that has already ended fails `CONFLICT`. */
  cancelRun(runId: string) {
    return fetchWithToken<Record<string, never>>(
      `${host()}/runs/${encodeURIComponent(runId)}/cancel`,
      { method: 'POST' }
    ).then((result) => result.map(() => undefined));
  },
};
