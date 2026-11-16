import { SERVER_HOSTS } from '@core/constant/servers';
import { fetchWithToken } from '@core/util/fetchWithToken';
import {
  getAnswerFactoryGateUrl,
  getCancelFactoryRunUrl,
  getCreateFactoryRunUrl,
  getGetFactoryRunUrl,
  getListFactoryFlowsUrl,
  getListFactoryTaskRunsUrl,
} from './generated/client';
import type {
  AnswerGateRequest,
  CreateRunRequest,
  FlowsResponse,
  RunDetail,
  TaskRunsResponse,
} from './generated/schemas';

const host = () => SERVER_HOSTS.factory;

/**
 * The software factory's Runs, as the signed-in person sees them: their own
 * Runs only. The routes and shapes are the service's generated client's.
 */
export const factoryClient = {
  /** The caller's Runs of a task, newest first; the service returns 50 at most. */
  listTaskRuns(taskId: string, signal?: AbortSignal) {
    return fetchWithToken<TaskRunsResponse>(
      `${host()}${getListFactoryTaskRunsUrl(encodeURIComponent(taskId))}`,
      { method: 'GET', signal }
    ).then((result) => result.map((data) => data.runs));
  },

  getRun(runId: string, signal?: AbortSignal) {
    return fetchWithToken<RunDetail>(
      `${host()}${getGetFactoryRunUrl(encodeURIComponent(runId))}`,
      { method: 'GET', signal }
    );
  },

  /** The Flows a Run can be started from. */
  listFlows(signal?: AbortSignal) {
    return fetchWithToken<FlowsResponse>(
      `${host()}${getListFactoryFlowsUrl()}`,
      {
        method: 'GET',
        signal,
      }
    ).then((result) => result.map((data) => data.flows));
  },

  /** Starts a Run of a Flow, by its name. */
  startRun(request: CreateRunRequest) {
    return fetchWithToken<RunDetail>(`${host()}${getCreateFactoryRunUrl()}`, {
      method: 'POST',
      body: JSON.stringify(request),
    });
  },

  /**
   * Answers the Gate of the Run's Step `seq`; the service answers 202 with
   * no body. A Gate that was already answered, or that the Run no longer
   * waits on, fails `CONFLICT`; an agent answering for its owner fails
   * `FORBIDDEN`.
   */
  answerGate(runId: string, seq: number, answer: AnswerGateRequest) {
    return fetchWithToken<Record<string, never>>(
      `${host()}${getAnswerFactoryGateUrl(encodeURIComponent(runId), seq)}`,
      { method: 'POST', body: JSON.stringify(answer) }
    ).then((result) => result.map(() => undefined));
  },

  /**
   * Cancels a Run (202, no body). Cancelling a cancelled Run is accepted
   * again; one that ended some other way fails `CONFLICT`.
   */
  cancelRun(runId: string) {
    return fetchWithToken<Record<string, never>>(
      `${host()}${getCancelFactoryRunUrl(encodeURIComponent(runId))}`,
      { method: 'POST' }
    ).then((result) => result.map(() => undefined));
  },
};
