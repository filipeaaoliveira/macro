import { SERVER_HOSTS } from '@core/constant/servers';
import {
  type FetchWithTokenErrorCode,
  fetchWithToken,
} from '@core/util/fetchWithToken';
import type { ResultError } from '@core/util/result';
import { statusError } from '@core/util/safeFetch';
import {
  getAnswerFactoryGateUrl,
  getCancelFactoryRunUrl,
  getCreateFactoryRunUrl,
  getGetFactoryInsightsUrl,
  getGetFactoryRunUrl,
  getListFactoryFlowsUrl,
  getListFactoryTaskRunsUrl,
} from './generated/client';
import type {
  AnswerGateRequest,
  CreateRunRequest,
  FlowsResponse,
  GetFactoryInsightsParams,
  RunDetail,
  RunErrorCode,
  RunInsights,
  TaskRunsResponse,
} from './generated/schemas';

const host = () => SERVER_HOSTS.factory;

/** Why a Run wasn't started, when the service names the cause. */
export type StartRunErrorCode = 'UNREACHABLE_REPOSITORY' | 'NO_PUSH_ACCESS';

const START_RUN_ERROR_CODES: Record<RunErrorCode, StartRunErrorCode> = {
  unreachableRepository: 'UNREACHABLE_REPOSITORY',
  noPushAccess: 'NO_PUSH_ACCESS',
};

function isRunErrorCode(code: unknown): code is RunErrorCode {
  return typeof code === 'string' && Object.hasOwn(START_RUN_ERROR_CODES, code);
}

/**
 * A refused start: the cause the service names in its body's `code`, else
 * the status's error.
 */
async function startRunError(
  response: Response
): Promise<ResultError<FetchWithTokenErrorCode | StartRunErrorCode>> {
  const failure = statusError(response.status);
  const body: unknown = await response.json().catch(() => undefined);
  if (!body || typeof body !== 'object' || !('code' in body)) return failure;
  if (!isRunErrorCode(body.code)) return failure;

  const message =
    'message' in body && typeof body.message === 'string'
      ? body.message
      : failure.message;
  return { code: START_RUN_ERROR_CODES[body.code], message };
}

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

  /**
   * How the caller's own Runs went over a period: the newest 1,000 of its
   * Runs, by day, outcome and Flow, with their Steps and agent usage.
   */
  getInsights(params: GetFactoryInsightsParams, signal?: AbortSignal) {
    return fetchWithToken<RunInsights>(
      `${host()}${getGetFactoryInsightsUrl(params)}`,
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

  /**
   * Starts a Run of a Flow, by its name. A repository the caller's linked
   * GitHub account can't reach fails `UNREACHABLE_REPOSITORY`, one it can't
   * push to `NO_PUSH_ACCESS`.
   */
  startRun(request: CreateRunRequest) {
    return fetchWithToken<RunDetail, StartRunErrorCode>(
      `${host()}${getCreateFactoryRunUrl()}`,
      {
        method: 'POST',
        body: JSON.stringify(request),
        errorResponseHandler: startRunError,
      }
    );
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
