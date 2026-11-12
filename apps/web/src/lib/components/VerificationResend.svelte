<script lang="ts">
  import { onMount } from 'svelte';
  import { refreshAll } from '$app/navigation';
  let { transport = 'sandbox' }: { transport?: 'sandbox' | 'smtp' } = $props();
  let busy = $state(false);
  let error = $state('');
  let message = $state('');
  let disposed = false;
  let controller: AbortController | undefined;
  onMount(() => () => {
    disposed = true;
    controller?.abort();
  });
  async function resend() {
    if (busy) return;
    busy = true;
    error = '';
    message = '';
    const current = new AbortController();
    controller = current;
    try {
      const response = await fetch('/api/platform/account/verification/resend', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
        signal: current.signal,
      });
      const result = await response.json();
      if (disposed || current.signal.aborted) return;
      if (!response.ok)
        throw new Error(
          result.message ??
            (response.status === 429
              ? 'Too many requests. Wait a little, then try again.'
              : 'Unable to resend verification email. Please try again.'),
        );
      message = result.message ?? 'If verification is still needed, a message has been queued.';
      await refreshAll();
    } catch (e) {
      if (!disposed && !current.signal.aborted)
        error =
          e instanceof Error ? e.message : 'Unable to resend verification email. Please try again.';
    } finally {
      if (!disposed) busy = false;
    }
  }
</script>

<div class="verification-resend" aria-busy={busy}>
  <p class="footnote">
    {transport === 'smtp'
      ? 'Check your email inbox for the verification message. Request a new message if the link has expired.'
      : 'Use the local sandbox inbox below to open your verification link. Local verification does not prove ownership of your email address.'}
  </p>
  {#if error}<p class="error-message" role="alert">{error}</p>{/if}
  {#if message}<p class="status-message" role="status">{message}</p>{/if}
  <button class="button-secondary" onclick={resend} disabled={busy}
    >{busy ? 'Sending verification email…' : 'Resend verification email'}</button
  >
</div>

<style>
  .verification-resend {
    margin-bottom: 20px;
  }
  .footnote {
    margin-bottom: 12px;
  }
</style>
