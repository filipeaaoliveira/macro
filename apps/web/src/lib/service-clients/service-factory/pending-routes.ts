/**
 * The shapes this client sends and reads that factory_service does not
 * publish yet: its Run routes are internal, and a person's routes (a task's
 * Runs, the Flows, a Run started from a Flow's name, an answer that takes
 * its user from the session) are still to come. Everything in this file goes
 * once the service's OpenAPI document has them: regenerate with
 * `bun run gen-api factory-service` and import the generated types instead.
 * `client.test.ts` checks the routes the client calls until then.
 */
import type {
  AnswerGateRequest,
  CreateRunRequest,
  RunDetail,
} from './generated/schemas';

/** `GET /tasks/{taskId}/runs`: the task's Runs, newest first. */
export type TaskRunsResponse = { runs: RunDetail[] };

/** A Flow a person can start a Run of: its name and what it does. */
export type FlowSummary = {
  name: string;
  description: string;
};

/** `GET /flows`. */
export type FlowsResponse = { flows: FlowSummary[] };

/** `POST /runs` with a Flow's name rather than its definition. */
export type StartRunRequest = Omit<CreateRunRequest, 'flow' | 'taskId'> & {
  flow: string;
  taskId: string;
};

/** `POST /runs/{id}/gates/{entry}/{visit}/answer` without `userId`. */
export type GateAnswerRequest = Omit<AnswerGateRequest, 'userId'>;
