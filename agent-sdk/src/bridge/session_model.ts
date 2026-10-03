import { readAppliedSettings } from "./query_settings.js";
import { emitCurrentModelUpdate, refreshCurrentModel, type SessionState } from "./session_lifecycle.js";
import { SessionObservations } from "./session_observations.js";

const observations = new SessionObservations();

export function observeSessionModel(session: SessionState, model: string): void {
  observations.begin(session);
  session.resolvedRuntimeModelId = model;
}

/** Publish the runtime model, including policy step-downs, rather than the requested alias. */
export async function refreshSessionModel(session: SessionState, emit = true): Promise<void> {
  const sessionId = session.sessionId;
  const current = observations.begin(session);
  let model: string | undefined;
  let failure: unknown;
  try {
    const applied = await readAppliedSettings(session.query);
    if (typeof applied.model !== "string" || applied.model.length === 0) throw new Error("No applied model was reported.");
    model = applied.model;
  } catch (error) { failure = error; }
  if (!current()) {
    if (emit && !session.closing && session.sessionId === sessionId) emitCurrentModelUpdate(session);
    return;
  }
  session.resolvedRuntimeModelId = model;
  refreshCurrentModel(session);
  if (emit) emitCurrentModelUpdate(session);
  if (failure) throw new Error("The current model could not be verified. Retry /model.", { cause: failure });
}
