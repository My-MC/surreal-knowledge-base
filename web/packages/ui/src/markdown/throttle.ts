export interface Throttler<A> {
  (value: A): void;
  cancel(): void;
}

/**
 * Trailing-edge throttle: the first call schedules a flush `waitMs` later;
 * calls arriving while a flush is pending only update the queued value. At
 * most one invocation per window, always with the most recent value.
 * `cancel()` drops a pending flush without invoking `fn` (used when a stream
 * ends so the final content can be rendered immediately by the caller).
 */
export function throttleTrailing<A>(fn: (value: A) => void, waitMs: number): Throttler<A> {
  let timer: ReturnType<typeof setTimeout> | null = null;
  let pending: { value: A } | null = null;

  const throttler = (value: A): void => {
    pending = { value };
    if (timer === null) {
      timer = setTimeout(() => {
        timer = null;
        const current = pending;
        pending = null;
        if (current !== null) fn(current.value);
      }, waitMs);
    }
  };

  throttler.cancel = (): void => {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
    pending = null;
  };

  return throttler;
}
