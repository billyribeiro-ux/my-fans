import { afterEach, describe, expect, it, vi } from 'vitest';
import { startPolling } from './polling.ts';
afterEach(() => vi.useRealTimers());
describe('visible bounded polling', () => {
  it('does not run disabled/hidden polling and resumes when available', async () => {
    vi.useFakeTimers();
    let visible = false;
    let enabled = true;
    const run = vi.fn(async () => {});
    const poll = startPolling({
      available: () => visible,
      enabled: () => enabled,
      run,
      onerror: vi.fn(),
      intervalMs: 1000,
    });
    poll.start();
    await vi.advanceTimersByTimeAsync(3000);
    expect(run).not.toHaveBeenCalled();
    visible = true;
    enabled = false;
    poll.resume();
    await vi.advanceTimersByTimeAsync(2000);
    expect(run).not.toHaveBeenCalled();
    enabled = true;
    poll.resume();
    await vi.advanceTimersByTimeAsync(0);
    expect(run).toHaveBeenCalledTimes(1);
    poll.stop();
  });
  it('backs off failures and returns to normal after success', async () => {
    vi.useFakeTimers();
    const error = vi.fn();
    const run = vi.fn().mockRejectedValueOnce(new Error('offline')).mockResolvedValue(undefined);
    const poll = startPolling({
      available: () => true,
      enabled: () => true,
      run,
      onerror: error,
      intervalMs: 1000,
      maxBackoffMs: 4000,
    });
    poll.start();
    await vi.advanceTimersByTimeAsync(1000);
    expect(error).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1000);
    expect(run).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(1000);
    expect(run).toHaveBeenCalledTimes(2);
    await vi.advanceTimersByTimeAsync(1000);
    expect(run).toHaveBeenCalledTimes(3);
    poll.stop();
  });
  it('aborts hidden/disposed work without overlaps or rescheduling', async () => {
    vi.useFakeTimers();
    let visible = true;
    let signal: AbortSignal | undefined;
    const run = vi.fn((current: AbortSignal) => {
      signal = current;
      return new Promise<void>((_resolve, reject) =>
        current.addEventListener('abort', () => reject(current.reason), { once: true }),
      );
    });
    const poll = startPolling({
      available: () => visible,
      enabled: () => true,
      run,
      onerror: vi.fn(),
      intervalMs: 1000,
    });
    poll.start();
    await vi.advanceTimersByTimeAsync(1000);
    expect(run).toHaveBeenCalledTimes(1);
    await vi.advanceTimersByTimeAsync(3000);
    expect(run).toHaveBeenCalledTimes(1);
    visible = false;
    poll.resume();
    expect(signal?.aborted).toBe(true);
    await vi.advanceTimersByTimeAsync(3000);
    expect(run).toHaveBeenCalledTimes(1);
    visible = true;
    poll.resume();
    await vi.advanceTimersByTimeAsync(0);
    expect(run).toHaveBeenCalledTimes(2);
    poll.stop();
    expect(signal?.aborted).toBe(true);
    await vi.advanceTimersByTimeAsync(10000);
    expect(run).toHaveBeenCalledTimes(2);
  });
  it('bounds request time and reports timeout for backoff', async () => {
    vi.useFakeTimers();
    const errors = vi.fn();
    const run = vi.fn(
      (signal: AbortSignal) =>
        new Promise<void>((_resolve, reject) =>
          signal.addEventListener('abort', () => reject(signal.reason), { once: true }),
        ),
    );
    const poll = startPolling({
      available: () => true,
      enabled: () => true,
      run,
      onerror: errors,
      intervalMs: 1000,
      timeoutMs: 2000,
    });
    poll.start();
    await vi.advanceTimersByTimeAsync(3000);
    expect(errors).toHaveBeenCalledTimes(1);
    expect(run).toHaveBeenCalledTimes(1);
    poll.stop();
  });
});
