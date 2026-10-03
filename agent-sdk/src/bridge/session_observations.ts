import type { SessionState } from "./session_lifecycle.js";

/** A later runtime observation or a replaced/closed session invalidates an older read. */
export class SessionObservations {
  private readonly generations = new WeakMap<SessionState, number>();

  begin(session: SessionState): () => boolean {
    const sessionId = session.sessionId;
    const generation = (this.generations.get(session) ?? 0) + 1;
    this.generations.set(session, generation);
    return () => !session.closing && session.sessionId === sessionId && this.generations.get(session) === generation;
  }
}
