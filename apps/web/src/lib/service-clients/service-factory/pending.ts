/**
 * Factory API fields and endpoints the generated client does not have yet.
 *
 * HAND-WRITTEN until the factory service's OpenAPI document, its client and
 * the SDK are regenerated. Each type mirrors the service's own; once
 * `generated/` has it, delete it here and read the generated one.
 */
import type { Run, Step } from './generated/schemas';

// --- Where each Step ran, and when a Run merged (Run reads) ---------------

/**
 * Where an agent Step's session ran (`StepRuntime`): the model, the Machine
 * the session first opened on, and the display name of the agent whose
 * session it is. Each is null for a Step no agent does, and for a Step from
 * before Steps kept it; `machine` is null too for anyone but the Run's
 * owner.
 */
export type StepRuntime = {
  model?: string | null;
  machine?: string | null;
  agentName?: string | null;
};

/** `Step`, with where it ran; a service older than it sends none. */
export type StepWithRuntime = Step & { runtime?: StepRuntime | null };

/** `Run`, with when its pull request was merged. */
export type RunWithMergedAt = Run & { mergedAt?: string | null };
