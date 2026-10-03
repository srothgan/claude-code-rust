/** Register before publishing, and settle exactly once on reply or native abort. */
export function waitForAnswer<T, P>(
  pending: Map<string, P>, id: string,
  entry: (settle: (value: T) => void) => P,
  publish: () => void,
  signal: AbortSignal | undefined,
  cancelled: () => T,
): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    let settled = false;
    let registered: P;
    const cleanup = () => {
      if (pending.get(id) === registered) pending.delete(id);
      signal?.removeEventListener("abort", abort);
    };
    const settle = (value: T) => {
      if (settled) return;
      settled = true;
      cleanup();
      resolve(value);
    };
    const abort = () => settle(cancelled());
    registered = entry(settle);
    if (signal?.aborted) { abort(); return; }
    pending.set(id, registered);
    signal?.addEventListener("abort", abort, { once: true });
    try {
      publish();
    } catch (error) {
      settled = true;
      cleanup();
      reject(error);
    }
  });
}
