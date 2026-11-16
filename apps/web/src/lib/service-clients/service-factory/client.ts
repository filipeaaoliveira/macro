import { SERVER_HOSTS } from '@core/constant/servers';
import { fetchWithToken } from '@core/util/fetchWithToken';
import type {
  AnswerGateRequest,
  CreateRunRequest,
  RunDetail,
} from './generated/schemas';

const host = () => SERVER_HOSTS.factory;

/** A Flow a person can start a Run of: its name and what it does. */
export type FlowSummary = {
  name: string;
  description: string;
};

/** Starts a Run of a named Flow on a task. */
export type StartRunRequest = Omit<CreateRunRequest, 'flow' | 'taskId'> & {
  flow: string;
  taskId: string;
};

/** A Gate's answer; the service records the signed-in person as who gave it. */
export type GateAnswerRequest = Omit<AnswerGateRequest, 'userId'>;

/** The software factory's Runs, as the signed-in person sees them. */
export const factoryClient = {
  /** The Runs of a task, newest first. */
  listTaskRuns(taskId: string, signal?: AbortSignal) {
    return fetchWithToken<{ runs: RunDetail[] }>(
      `${host()}/tasks/${taskId}/runs`,
      { method: 'GET', signal }
    ).then((result) => result.map((data) => data.runs));
  },

  getRun(runId: string, signal?: AbortSignal) {
    return fetchWithToken<RunDetail>(`${host()}/runs/${runId}`, {
      method: 'GET',
      signal,
    });
  },

  /** The Flows a Run can be started from. */
  listFlows(signal?: AbortSignal) {
    return fetchWithToken<{ flows: FlowSummary[] }>(`${host()}/flows`, {
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
      `${host()}/runs/${runId}/gates/${entry}/${visit}/answer`,
      { method: 'POST', body: JSON.stringify(answer) }
    ).then((result) => result.map(() => undefined));
  },

  /** Cancels a Run. One that has already ended fails `CONFLICT`. */
  cancelRun(runId: string) {
    return fetchWithToken<Record<string, never>>(
      `${host()}/runs/${runId}/cancel`,
      { method: 'POST' }
    ).then((result) => result.map(() => undefined));
  },
};
