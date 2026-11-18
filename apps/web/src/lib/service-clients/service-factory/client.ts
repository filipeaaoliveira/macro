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
  getCreateFactoryAutomationUrl,
  getCreateFactoryRunUrl,
  getDeleteFactoryAutomationUrl,
  getFlawFactoryRunUrl,
  getGetFactoryControlFilesUrl,
  getGetFactoryInsightsUrl,
  getGetFactoryRunUrl,
  getGetFactorySessionStepUrl,
  getGetFactoryStandingRulesUrl,
  getListFactoryAutomationsUrl,
  getListFactoryBoardUrl,
  getListFactoryFlowsUrl,
  getListFactoryTaskRunsUrl,
  getListFactoryWaitingRunsUrl,
  getNoteFactoryStepUrl,
  getPauseFactoryStepUrl,
  getReplyToFactoryGateUrl,
  getResumeFactoryStepUrl,
  getSetFactoryAutomationEnabledUrl,
  getSetFactoryControlFilesUrl,
  getSetFactoryStandingRulesUrl,
  getStopFactoryStepUrl,
} from './generated/client';
import type {
  AnswerGateRequest,
  Automation,
  AutomationRequest,
  AutomationsSettings,
  BoardPage,
  ControlFilesSettings,
  CreatedAutomation,
  CreateRunRequest,
  FlawRunRequest,
  FlowsResponse,
  GetFactoryInsightsParams,
  ListFactoryBoardParams,
  NoteStepRequest,
  ReplyToGateRequest,
  ResumeStepRequest,
  RunDetail,
  RunErrorCode,
  RunInsights,
  SessionStepRef,
  StandingRulesSettings,
  TaskRunsResponse,
  WaitingRunsResponse,
} from './generated/schemas';

const host = () => SERVER_HOSTS.factory;

/** Why a Run wasn't started, when the service names the cause. */
export type StartRunErrorCode =
  | 'UNREACHABLE_REPOSITORY'
  | 'NO_PUSH_ACCESS'
  | 'QUEUE_FULL';

/** The codes the service refuses a start with. */
type StartRunRefusal = Exclude<
  RunErrorCode,
  'controlFilesNotApproved' | 'reservedForAPerson'
>;

const START_RUN_ERROR_CODES: Record<StartRunRefusal, StartRunErrorCode> = {
  unreachableRepository: 'UNREACHABLE_REPOSITORY',
  noPushAccess: 'NO_PUSH_ACCESS',
  queueFull: 'QUEUE_FULL',
};

function isRunErrorCode(code: unknown): code is StartRunRefusal {
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
 * A refusal that says why in its body's `message`, else the status's error.
 */
async function refusal(
  response: Response
): Promise<ResultError<FetchWithTokenErrorCode>> {
  const failure = statusError(response.status);
  const body: unknown = await response.json().catch(() => undefined);
  if (!body || typeof body !== 'object' || !('message' in body)) {
    return failure;
  }
  const message =
    typeof body.message === 'string' ? body.message : failure.message;
  return { code: failure.code, message };
}

/**
 * The software factory's Runs, as the signed-in person sees them: their own,
 * and on the board and a Run's page their team's on tasks they can see. The routes and shapes are the service's generated client's.
 */
export const factoryClient = {
  /** The caller's Runs of a task, newest first; the service returns 50 at most. */
  listTaskRuns(taskId: string, signal?: AbortSignal) {
    return fetchWithToken<TaskRunsResponse>(
      `${host()}${getListFactoryTaskRunsUrl(encodeURIComponent(taskId))}`,
      { method: 'GET', signal }
    ).then((result) => result.map((data) => data.runs));
  },

  /**
   * The caller's Runs that hold a place in their Queue, newest first: those
   * waiting on them at a Gate (`runs`), and those yet to reach their first
   * (`starting`).
   */
  listWaitingRuns(signal?: AbortSignal) {
    return fetchWithToken<WaitingRunsResponse>(
      `${host()}${getListFactoryWaitingRunsUrl()}`,
      { method: 'GET', signal }
    );
  },

  /**
   * A page of the board: the caller's Runs, or their team's on tasks they
   * can see, newest first, each with where it is; `next` starts the page
   * after.
   */
  listBoard(params: ListFactoryBoardParams, signal?: AbortSignal) {
    return fetchWithToken<BoardPage>(
      `${host()}${getListFactoryBoardUrl(params)}`,
      { method: 'GET', signal }
    );
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
   * `FORBIDDEN`. Approving a Tampered Run without `approvedControlFiles`
   * listing its Tampered files fails `BAD_REQUEST`.
   */
  answerGate(runId: string, seq: number, answer: AnswerGateRequest) {
    return fetchWithToken<Record<string, never>>(
      `${host()}${getAnswerFactoryGateUrl(encodeURIComponent(runId), seq)}`,
      { method: 'POST', body: JSON.stringify(answer) }
    ).then((result) => result.map(() => undefined));
  },

  /**
   * Gives the Gate of the Run's Step `seq` the person's words, which the
   * factory reads as one of its answers or asks back on.
   */
  replyToGate(runId: string, seq: number, reply: ReplyToGateRequest) {
    return fetchWithToken<Record<string, never>>(
      `${host()}${getReplyToFactoryGateUrl(encodeURIComponent(runId), seq)}`,
      { method: 'POST', body: JSON.stringify(reply) }
    ).then((result) => result.map(() => undefined));
  },

  /**
   * The person's factory Step that works in the agent session `sessionId`;
   * any other session fails `NOT_FOUND`.
   */
  getSessionStep(sessionId: string, signal?: AbortSignal) {
    return fetchWithToken<SessionStepRef>(
      `${host()}${getGetFactorySessionStepUrl(encodeURIComponent(sessionId))}`,
      { method: 'GET', signal }
    );
  },

  /**
   * Sends the agent of the Run's running Step `seq` a note, from its Step
   * Terminal. A Step that ended, is paused, or whose agent works on no turn
   * fails `CONFLICT`; only the Run's owner, signed in, may (`FORBIDDEN`).
   */
  noteStep(runId: string, seq: number, note: NoteStepRequest) {
    return fetchWithToken<Record<string, never>>(
      `${host()}${getNoteFactoryStepUrl(encodeURIComponent(runId), seq)}`,
      { method: 'POST', body: JSON.stringify(note) }
    ).then((result) => result.map(() => undefined));
  },

  /** Pauses the Run's running Step `seq`: its agent's turn stops. */
  pauseStep(runId: string, seq: number) {
    return fetchWithToken<Record<string, never>>(
      `${host()}${getPauseFactoryStepUrl(encodeURIComponent(runId), seq)}`,
      { method: 'POST' }
    ).then((result) => result.map(() => undefined));
  },

  /** Resumes the Run's paused Step `seq`, with the person's note if any. */
  resumeStep(runId: string, seq: number, resume: ResumeStepRequest) {
    return fetchWithToken<Record<string, never>>(
      `${host()}${getResumeFactoryStepUrl(encodeURIComponent(runId), seq)}`,
      { method: 'POST', body: JSON.stringify(resume) }
    ).then((result) => result.map(() => undefined));
  },

  /** Stops the Run's running Step `seq`: it ends `failed`. */
  stopStep(runId: string, seq: number) {
    return fetchWithToken<Record<string, never>>(
      `${host()}${getStopFactoryStepUrl(encodeURIComponent(runId), seq)}`,
      { method: 'POST' }
    ).then((result) => result.map(() => undefined));
  },

  /**
   * The Control File patterns the person's Runs are checked against: their
   * team's or the default, and whether the person may change them.
   */
  getControlFiles(signal?: AbortSignal) {
    return fetchWithToken<ControlFilesSettings>(
      `${host()}${getGetFactoryControlFilesUrl()}`,
      { method: 'GET', signal }
    );
  },

  /**
   * Sets the team's Control File patterns; `null` goes back to the default.
   * Someone who doesn't administer the team fails `FORBIDDEN`; a pattern
   * that doesn't read fails `HTTP_ERROR` (400). Either carries the
   * service's message.
   */
  setControlFiles(patterns: string[] | null) {
    return fetchWithToken<ControlFilesSettings>(
      `${host()}${getSetFactoryControlFilesUrl()}`,
      {
        method: 'PUT',
        body: JSON.stringify({ patterns }),
        errorResponseHandler: refusal,
      }
    );
  },

  /**
   * The Standing Rules the person's Runs start with: which kinds of Gate
   * the factory answers on its own, their team's or the default, and
   * whether the person may change them.
   */
  getStandingRules(signal?: AbortSignal) {
    return fetchWithToken<StandingRulesSettings>(
      `${host()}${getGetFactoryStandingRulesUrl()}`,
      { method: 'GET', signal }
    );
  },

  /**
   * Sets the team's Standing Rules; `null` goes back to the default. Someone
   * who doesn't administer the team fails `FORBIDDEN`, with the service's
   * message.
   */
  setStandingRules(definition: StandingRulesSettings['definition'] | null) {
    return fetchWithToken<StandingRulesSettings>(
      `${host()}${getSetFactoryStandingRulesUrl()}`,
      {
        method: 'PUT',
        body: JSON.stringify({ definition }),
        errorResponseHandler: refusal,
      }
    );
  },

  /**
   * The person's team's Automations, newest first, and whether they
   * administer the team.
   */
  listAutomations(signal?: AbortSignal) {
    return fetchWithToken<AutomationsSettings>(
      `${host()}${getListFactoryAutomationsUrl()}`,
      { method: 'GET', signal }
    );
  },

  /**
   * Makes an Automation of the person's team, acting as them. A webhook's
   * secret is in the answer, once. A refusal (a schedule, a repository or a
   * trigger the factory or its scheduler won't take) carries the service's
   * message.
   */
  createAutomation(request: AutomationRequest) {
    return fetchWithToken<CreatedAutomation>(
      `${host()}${getCreateFactoryAutomationUrl()}`,
      {
        method: 'POST',
        body: JSON.stringify(request),
        errorResponseHandler: refusal,
      }
    );
  },

  /**
   * Switches an Automation on or off: the person's own, or any for a team
   * admin (`FORBIDDEN` otherwise, with the service's message).
   */
  setAutomationEnabled(id: string, enabled: boolean) {
    return fetchWithToken<Automation>(
      `${host()}${getSetFactoryAutomationEnabledUrl(encodeURIComponent(id))}`,
      {
        method: 'PUT',
        body: JSON.stringify({ enabled }),
        errorResponseHandler: refusal,
      }
    );
  },

  /** Deletes an Automation, as for switching it off (204, no body). */
  deleteAutomation(id: string) {
    return fetchWithToken<Record<string, never>>(
      `${host()}${getDeleteFactoryAutomationUrl(encodeURIComponent(id))}`,
      { method: 'DELETE', errorResponseHandler: refusal }
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

  /**
   * Marks one of the caller's Runs that ended in success flawed, with why:
   * it reads `flawed` after, with who marked it. 409 when it did not end in
   * success.
   */
  flawRun(runId: string, body: FlawRunRequest) {
    return fetchWithToken<RunDetail>(
      `${host()}${getFlawFactoryRunUrl(encodeURIComponent(runId))}`,
      { method: 'POST', body: JSON.stringify(body) }
    );
  },
};
