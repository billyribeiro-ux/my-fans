export interface PollOptions {
  enabled: () => boolean;
  available: () => boolean;
  run: (signal: AbortSignal) => Promise<void>;
  onerror: (error: unknown) => void;
  intervalMs?: number;
  timeoutMs?: number;
  maxBackoffMs?: number;
}
// Created by onMount; availability events are owned by <svelte:document/window>.
// No SSR timers, overlapping requests, or credential state in this helper.
export function startPolling({
  enabled,
  available,
  run,
  onerror,
  intervalMs = 12000,
  timeoutMs = 15000,
  maxBackoffMs = 60000,
}: PollOptions) {
  let timer: ReturnType<typeof setTimeout> | undefined;
  let controller: AbortController | undefined;
  let stopped = false;
  let running = false;
  let suspended = false;
  let generation = 0;
  let delay = intervalMs;
  function schedule(wait = delay) {
    if (timer) clearTimeout(timer);
    timer = undefined;
    if (!stopped && !suspended && available())
      timer = setTimeout(() => {
        void tick();
      }, wait);
  }
  async function tick() {
    timer = undefined;
    if (stopped || suspended || !available()) return;
    if (running || !enabled()) {
      schedule();
      return;
    }
    const current = new AbortController();
    controller = current;
    const version = generation;
    running = true;
    const timeout = setTimeout(
      () => current.abort(new DOMException('Updates timed out.', 'TimeoutError')),
      timeoutMs,
    );
    try {
      await run(current.signal);
      if (!stopped && version === generation) delay = intervalMs;
    } catch (error) {
      if (
        !stopped &&
        version === generation &&
        (!current.signal.aborted || current.signal.reason?.name === 'TimeoutError')
      ) {
        delay = Math.min(maxBackoffMs, delay * 2);
        onerror(error);
      }
    } finally {
      clearTimeout(timeout);
      if (controller === current) controller = undefined;
      running = false;
      if (!stopped && version === generation) schedule();
    }
  }
  return {
    pause() {
      suspended = true;
      generation++;
      if (timer) clearTimeout(timer);
      timer = undefined;
      controller?.abort();
    },
    resume() {
      suspended = false;
      generation++;
      if (timer) clearTimeout(timer);
      timer = undefined;
      controller?.abort();
      if (available()) {
        delay = intervalMs;
        schedule(0);
      }
    },
    stop() {
      stopped = true;
      generation++;
      if (timer) clearTimeout(timer);
      timer = undefined;
      controller?.abort();
      controller = undefined;
    },
    start() {
      suspended = false;
      schedule(intervalMs);
    },
  };
}
