<script lang="ts">
  import { onMount } from 'svelte';
  import { refreshAll } from '$app/navigation';
  import { ShieldCheck } from '@lucide/svelte';
  import Dialog from './Dialog.svelte';
  import type { CreatorOnboarding } from '#lib/creator-onboarding.ts';
  let {
    userId,
    onboarding,
    identityMode,
    paymentMode,
  }: {
    userId: string;
    onboarding?: CreatorOnboarding | null;
    identityMode?: 'sandbox' | 'sumsub';
    paymentMode?: 'sandbox' | 'stripe';
  } = $props();
  let progress = $derived(onboarding?.viewer_id === userId ? onboarding : null);
  let confirming = $state(false);
  let reason = $state('');
  let busy = $state(false);
  let error = $state('');
  let message = $state('');
  let disposed = false;
  const controller = new AbortController();
  onMount(() => () => {
    disposed = true;
    controller.abort();
    reason = '';
  });
  function close() {
    confirming = false;
    reason = '';
    error = '';
  }
  function checkedAt(value: string | null) {
    if (!value) return 'No provider update received yet.';
    const date = new Date(value);
    return Number.isFinite(date.getTime())
      ? `Last update: ${date.toLocaleString()}`
      : 'Update time is unavailable.';
  }
  async function resubmit(event: SubmitEvent) {
    event.preventDefault();
    if (busy || !progress?.review.resubmission_available) return;
    busy = true;
    error = '';
    message = '';
    try {
      const response = await fetch('/api/platform/creators/resubmit', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ reason: reason.trim() }),
        signal: controller.signal,
      });
      const result = await response.json();
      if (!response.ok)
        throw new Error(result.message ?? 'Unable to request another creator review.');
      if (result.status !== 'pending')
        throw new Error('Review status could not be confirmed. Refresh before trying again.');
      if (disposed) return;
      close();
      message = 'Creator review requested. Approval remains pending.';
      await refreshAll();
    } catch (e) {
      if (!disposed)
        error = e instanceof Error ? e.message : 'Unable to request another creator review.';
    } finally {
      if (!disposed) busy = false;
    }
  }
</script>

<section class="panel onboarding-progress" aria-label="Creator setup progress">
  <h2><ShieldCheck size={18} /> Creator setup progress</h2>
  {#if progress}
    <p class="section-description">
      These are the latest stored account and review results. Hosted provider updates can take time
      to arrive.
    </p>
    <dl>
      <div>
        <dt>Email</dt>
        <dd>
          <p class="progress-status">
            {progress.email_verified ? 'Verified' : 'Verification needed'}
          </p>
          {#if !progress.email_verified}<a class="text-button" href="/settings"
              >Verify your email in Settings</a
            >{/if}
        </dd>
      </div>
      <div>
        <dt>Authenticator</dt>
        <dd>
          <p class="progress-status">
            {progress.mfa_recovery_required
              ? 'Replacement required'
              : progress.mfa_enabled
                ? 'Enabled'
                : 'Setup needed for hosted onboarding'}
          </p>
          <a class="text-button" href="/settings#mfa-heading"
            >{progress.mfa_enabled && !progress.mfa_recovery_required
              ? 'Refresh authenticator verification'
              : 'Set up your authenticator'}</a
          >
          <p class="footnote">
            Hosted setup and review requests require recent authenticator verification.
          </p>
        </dd>
      </div>
      <div>
        <dt>Identity</dt>
        <dd>
          <p class="progress-status">
            {identityMode === 'sandbox'
              ? 'Sandbox review; no identity verification'
              : progress.identity.status.replaceAll('_', ' ')}
          </p>
          {#if identityMode !== 'sandbox'}<p class="footnote">
              {checkedAt(progress.identity.checked_at)}
            </p>{/if}
        </dd>
      </div>
      <div>
        <dt>Payment account</dt>
        <dd>
          {#if paymentMode === 'sandbox'}<p class="progress-status">
              Sandbox payments; no bank account connection
            </p>
          {:else}<p class="progress-status">
              {progress.payments.connected ? 'Connected' : 'Not connected'}
            </p>
            <p class="footnote">
              Charges: {progress.payments.charges_enabled ? 'Enabled' : 'Not enabled'} · Bank payouts:
              {progress.payments.payouts_enabled ? 'Enabled' : 'Not enabled'}
            </p>
            <p class="footnote">{checkedAt(progress.payments.checked_at)}</p>{/if}
        </dd>
      </div>
      <div>
        <dt>Platform creator review</dt>
        <dd>
          <p class="progress-status">{progress.review.status}</p>
          {#if progress.review.reason}<p class="review-reason">
              <strong>Review reason:</strong>
              {progress.review.reason}
            </p>{:else if progress.review.status === 'rejected'}<p class="footnote">
              A creator-facing review reason is unavailable. Contact support for further context or
              request another review.
            </p>{/if}
          {#if progress.review.reviewed_at}<p class="footnote">
              {checkedAt(progress.review.reviewed_at)}
            </p>{/if}
          {#if progress.review.resubmission_available}<button
              class="button-secondary"
              disabled={busy}
              onclick={() => {
                confirming = true;
                reason = '';
                error = '';
              }}>Request another creator review</button
            >{/if}
        </dd>
      </div>
    </dl>
    <p class="footnote">
      Creator approval, identity verification, and payment readiness are separate. Returning from a
      hosted flow does not approve any of them.
    </p>
  {:else}<p class="section-description">
      Creator progress is unavailable. Refresh the page before continuing with setup.
    </p>{/if}
  {#if message}<p class="status-message" role="status">{message}</p>{/if}
  {#if error && !confirming}<p class="error-message" role="alert">{error}</p>{/if}
</section>
{#if confirming}
  <Dialog title="Request another creator review" onclose={close}>
    <form class="form-grid" onsubmit={resubmit}>
      <p>
        Explain what you changed or why the previous decision should be reviewed. This request
        returns your creator profile to review; it does not approve identity, payments, or
        publication.
      </p>
      <p class="footnote">
        Verify your email and refresh authenticator verification in Settings before submitting.
      </p>
      <label
        >Review request reason<textarea bind:value={reason} required minlength="5" maxlength="2000"
        ></textarea></label
      >
      {#if error}<p class="error-message" role="alert">{error}</p>{/if}
      <div class="form-actions">
        <button class="button-primary" disabled={busy || reason.trim().length < 5}
          >{busy ? 'Submitting…' : 'Submit review request'}</button
        ><button class="button-secondary" type="button" disabled={busy} onclick={close}
          >Cancel</button
        >
      </div>
    </form>
  </Dialog>
{/if}

<style>
  h2 {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  dl {
    display: grid;
    gap: 18px;
    margin-bottom: 20px;
  }
  dt {
    font-size: 12px;
    font-weight: 700;
  }
  dd {
    margin: 4px 0 8px;
    font-size: 13px;
  }
  .progress-status {
    text-transform: capitalize;
  }
  .review-reason {
    font-size: 12px;
    overflow-wrap: anywhere;
  }
  .button-secondary {
    margin-top: 12px;
  }
</style>
