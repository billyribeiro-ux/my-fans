<script lang="ts">
  import { onMount } from 'svelte';
  import { goto } from '$app/navigation';
  import Dialog from './Dialog.svelte';
  import { privacyReceipt } from '#lib/privacy.ts';
  let {
    mfaEnabled = false,
    administrator = false,
  }: { mfaEnabled?: boolean; administrator?: boolean } = $props();
  let open = $state(false);
  let busy = $state(false);
  let password = $state('');
  let code = $state('');
  let confirmation = $state('');
  let error = $state('');
  let receipt = $state.raw<{ id: string; status: 'pending' } | null>(null);
  let controller: AbortController | undefined;
  let disposed = false;
  function clearCredentials() {
    password = '';
    code = '';
    confirmation = '';
  }
  onMount(() => () => {
    disposed = true;
    controller?.abort();
    clearCredentials();
    receipt = null;
  });
  async function leave() {
    clearCredentials();
    receipt = null;
    open = false;
    await goto('/auth?erasure=requested', { refreshAll: true });
  }
  function close() {
    if (busy) return;
    if (receipt) {
      void leave();
      return;
    }
    open = false;
    clearCredentials();
    error = '';
  }
  function downloadReceipt() {
    if (!receipt) return;
    const url = URL.createObjectURL(
      new Blob([JSON.stringify({ request_id: receipt.id, status: receipt.status }, null, 2)], {
        type: 'application/json',
      }),
    );
    const link = document.createElement('a');
    link.href = url;
    link.download = 'myfans-erasure-request.json';
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy || confirmation !== 'ERASE') return;
    busy = true;
    error = '';
    const request = new AbortController();
    controller = request;
    try {
      const response = await fetch('/api/platform/privacy/erasure', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          current_password: password,
          ...(mfaEnabled ? { mfa_code: code } : {}),
          confirmation,
        }),
        signal: request.signal,
      });
      const result = await response.json();
      if (disposed || request.signal.aborted) return;
      clearCredentials();
      if (!response.ok)
        throw new Error(
          result.message ?? 'The erasure request could not be confirmed. Please try again.',
        );
      receipt = privacyReceipt(result);
    } catch (e) {
      if (!disposed && !request.signal.aborted)
        error =
          e instanceof Error
            ? e.message
            : 'The request could not be confirmed. Contact support if you have been signed out.';
    } finally {
      if (!disposed) {
        busy = false;
        controller = undefined;
      }
    }
  }
</script>

<section class="panel">
  <h2>Request account erasure</h2>
  <p class="section-description">
    Request erasure under the <a class="text-button" href="/legal#privacy">privacy policy</a>. Your
    account will be disabled, sessions signed out, and posts hidden when the request is accepted.
    Subscription cancellations and financial obligations must be resolved before erasure proceeds.
  </p>
  <p class="section-description">
    Application data, historical storage versions, backups, and provider records follow separate
    review and retention steps. A submitted request does not mean these records have already been
    deleted. Export your data and approved media first.
  </p>
  {#if administrator}<p class="footnote">
      Administrator offboarding requires an operator-managed process.
    </p>{:else}<button
      class="button-secondary"
      onclick={() => {
        open = true;
        error = '';
        clearCredentials();
      }}>Request erasure</button
    >{/if}
</section>
{#if open}<Dialog
    title={receipt ? 'Erasure request received' : 'Request account erasure?'}
    onclose={close}
  >
    {#if receipt}<div class="receipt">
        <p>
          Your account has been disabled and its sessions revoked. Keep this request reference when
          contacting support.
        </p>
        <p class="request-reference">{receipt.id}</p>
        <p>
          This is a pending request. It does not certify erasure of application data, backups,
          historical versions, or provider records.
        </p>
        <button class="button-secondary" onclick={downloadReceipt}>Download request receipt</button
        ><button class="button-primary" onclick={leave}>Continue to sign in</button>
      </div>
    {:else}<form class="form-grid" onsubmit={submit}>
        <p>
          This disables your account immediately after the server accepts the request. Erasure is
          reviewed separately and retained financial, security, policy, or moderation evidence may
          remain under the published privacy policy.
        </p>
        <p>
          <a class="text-button" href="/legal#privacy" target="_blank" rel="noopener noreferrer"
            >Read the privacy policy</a
          > before continuing.
        </p>
        {#if error}<p class="error-message" role="alert">{error}</p>{/if}<label
          >Erasure current password<input
            type="password"
            autocomplete="current-password"
            bind:value={password}
            maxlength="128"
            required
          /></label
        >{#if mfaEnabled}<label
            >Erasure authenticator code<input
              bind:value={code}
              inputmode="numeric"
              autocomplete="one-time-code"
              pattern={'[0-9]{6}'}
              maxlength="6"
              required
            /></label
          >{/if}<label
          >Type ERASE to confirm<input
            bind:value={confirmation}
            autocomplete="off"
            maxlength="5"
            required
            pattern="ERASE"
          /></label
        >
        <div class="form-actions">
          <button class="button-primary" disabled={busy || confirmation !== 'ERASE'}
            >{busy ? 'Submitting…' : 'Confirm erasure request'}</button
          ><button type="button" class="button-secondary" disabled={busy} onclick={close}
            >Keep account active</button
          >
        </div>
      </form>{/if}
  </Dialog>{/if}

<style>
  .receipt {
    display: grid;
    gap: 16px;
    font-size: 13px;
    line-height: 1.8;
  }
  .request-reference {
    font-family: monospace;
    overflow-wrap: anywhere;
    padding: 12px;
    border-radius: 8px;
    background: var(--background);
  }
</style>
