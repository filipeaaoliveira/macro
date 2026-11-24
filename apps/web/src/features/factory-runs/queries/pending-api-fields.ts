import type {
  Run as ApiRun,
  Step as ApiStep,
} from '@service-factory/generated/schemas';

/*
 * HAND-WRITTEN API TYPES, PENDING REGENERATION.
 *
 * The factory API sends these fields, but the generated client
 * (`@service-factory/generated`) predates them. Regenerating the client from
 * the OpenAPI spec adds them to `Step` and `Run`; then delete this file and
 * read them from the generated types.
 */

/**
 * Where an agent Step's session ran (`StepRuntime`): the model, the Machine,
 * and the display name of the agent whose session it is. Each is null for a
 * Step no agent does, and for a Step from before Steps kept it.
 */
export type ApiStepRuntime = {
  model?: string | null;
  machine?: string | null;
  agentName?: string | null;
};

/** A Step as the API sends it, with its runtime; a service older than it sends none. */
export type ApiStepWithRuntime = ApiStep & { runtime?: ApiStepRuntime | null };

/** A Run as the API sends it, with when its pull request was merged. */
export type ApiRunWithMergedAt = ApiRun & { mergedAt?: string | null };
