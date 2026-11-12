<script lang="ts">
  import { onMount } from 'svelte';
  import { refreshAll } from '$app/navigation';
  import { ArrowUpRight, ShieldCheck } from '@lucide/svelte';
  import Dialog from './Dialog.svelte';
  import { hostedUrl, parseIntegrationStatus, type IntegrationStatus } from '#lib/integrations.ts';
  let {
    status = null,
    initialError = '',
    allowSetup = false,
    paymentsOnly = false,
  }: {
    status?: IntegrationStatus | null;
    initialError?: string | null;
    allowSetup?: boolean;
    paymentsOnly?: boolean;
  } = $props();
  let current = $state.raw<IntegrationStatus | null>(null);
  let active = $derived(current ?? status);
  let error = $state('');
  let busy = $state(false);
  let destination = $state<'payout-account' | 'payout-dashboard' | 'verification' | null>(null);
  let url = $state('');
  let disposed = false;
  const controller = new AbortController();
  onMount(() => () => {
    disposed = true;
    controller.abort();
    url = '';
  });
  async function reload() {
    if (busy) return;
    busy = true;
    error = '';
    current = null;
    try {
      const response = await fetch('/api/platform/integrations/status', {
        signal: controller.signal,
      });
      const result = await response.json();
      if (!response.ok) throw new Error(result.message ?? 'Provider status is unavailable.');
      if (disposed) return;
      current = parseIntegrationStatus(result);
      await refreshAll();
    } catch (e) {
      if (!disposed) error = e instanceof Error ? e.message : 'Provider status is unavailable.';
    } finally {
      if (!disposed) busy = false;
    }
  }
  async function prepare(kind: 'payout-account' | 'payout-dashboard' | 'verification') {
    if (busy || !allowSetup) return;
    busy = true;
    error = '';
    url = '';
    destination = null;
    try {
      const response = await fetch(`/api/platform/creators/${kind}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
        signal: controller.signal,
      });
      const result = await response.json();
      if (!response.ok) throw new Error(result.message ?? 'Unable to open provider setup.');
      const checked = hostedUrl(result.url, kind === 'payout-dashboard' ? 'payout-account' : kind);
      if (disposed) return;
      url = checked;
      destination = kind;
    } catch (e) {
      if (!disposed) error = e instanceof Error ? e.message : 'Unable to open provider setup.';
    } finally {
      if (!disposed) busy = false;
    }
  }
  function close() {
    url = '';
    destination = null;
  }
  function continueToProvider() {
    if (!destination || !url) return;
    const checked = hostedUrl(
      url,
      destination === 'payout-dashboard' ? 'payout-account' : destination,
    );
    close();
    window.location.assign(checked);
  }
</script>

<section
  class="panel provider-panel"
  aria-label={paymentsOnly ? 'Payout provider' : 'Payments and identity'}
>
  <h2>
    {paymentsOnly
      ? active?.payments === 'stripe'
        ? 'Manage Stripe payouts'
        : 'Payout provider'
      : 'Payments and identity'}
  </h2>
  {#if active}
    <p class="section-description">
      General creator content only. Explicit adult content is not supported.
    </p>
    <div class="provider-row">
      <ShieldCheck size={18} />
      <div>
        <strong>Payments: {active.payments === 'stripe' ? 'Stripe' : 'Sandbox'}</strong>
        <p>
          {active.payments === 'sandbox'
            ? 'Payments and balances are simulated. No real money is charged.'
            : 'Checkout and bank payouts are handled by Stripe.'}
        </p>
      </div>
    </div>
    {#if active.payments === 'stripe'}
      {#if !active.configured.payments}<p class="error-message" role="alert">
          Stripe setup is currently unavailable. Refresh or contact support before continuing.
        </p>
      {:else if allowSetup}<div class="provider-actions">
          <button class="button-secondary" disabled={busy} onclick={() => prepare('payout-account')}
            >Set up or update Stripe payouts</button
          >
          <button class="text-button" disabled={busy} onclick={() => prepare('payout-dashboard')}
            >Open Stripe dashboard <ArrowUpRight size={14} /></button
          >
        </div>
      {:else}<a class="text-button" href="/studio"
          >Manage creator payment setup in Studio <ArrowUpRight size={14} /></a
        >{/if}
    {/if}
    {#if !paymentsOnly}
      <div class="provider-row">
        <ShieldCheck size={18} />
        <div>
          <strong>Identity: {active.identity === 'sumsub' ? 'Sumsub' : 'Sandbox review'}</strong>
          <p>
            {active.identity === 'sandbox'
              ? 'Sandbox review does not constitute identity verification.'
              : 'Submit identity documents directly through Sumsub’s hosted verification.'}
          </p>
        </div>
      </div>
      {#if active.identity === 'sumsub'}
        {#if !active.configured.identity}<p class="error-message" role="alert">
            Identity verification is currently unavailable. Refresh or contact support before
            continuing.
          </p>
        {:else if allowSetup}<button
            class="button-secondary"
            disabled={busy}
            onclick={() => prepare('verification')}>Start identity verification</button
          >
        {:else}<a class="text-button" href="/studio"
            >Manage creator verification in Studio <ArrowUpRight size={14} /></a
          >{/if}
      {/if}
    {/if}
    <p class="footnote">
      Returning from a provider does not confirm payment, onboarding, or identity approval. Refresh
      to check the latest server status.
    </p>
  {:else}<p class="section-description">
      Provider status is unavailable. Refresh before starting payment or identity setup.
    </p>{/if}
  {#if error || (!active && initialError)}<p class="error-message" role="alert">
      {error || initialError}
    </p>{/if}
  <button class="text-button refresh-provider" disabled={busy} onclick={reload}
    >{busy ? 'Loading…' : 'Refresh provider status'}</button
  >
</section>
{#if destination && url}
  <Dialog
    title={destination === 'verification' ? 'Continue to Sumsub' : 'Continue to Stripe'}
    onclose={close}
  >
    <p class="section-description">
      You’ll leave MyFans and continue on {destination === 'verification'
        ? 'inquiry.sumsub.com'
        : 'connect.stripe.com'}. {destination === 'verification'
        ? 'Sumsub collects your verification documents.'
        : 'Stripe manages your payment account and bank details.'}
    </p>
    <p class="footnote">Completing the hosted flow does not itself confirm approval in MyFans.</p>
    <button class="button-primary full-width" onclick={continueToProvider}
      >Continue to {destination === 'verification' ? 'identity verification' : 'Stripe Connect'}
      <ArrowUpRight size={16} /></button
    >
  </Dialog>
{/if}

<style>
  .provider-row {
    display: flex;
    align-items: flex-start;
    gap: 12px;
    margin: 20px 0;
  }
  .provider-row strong {
    font-size: 13px;
  }
  .provider-row p {
    color: var(--muted);
    font-size: 12px;
    line-height: 1.8;
    margin-top: 5px;
  }
  .provider-actions {
    display: flex;
    gap: 14px;
    flex-wrap: wrap;
    align-items: center;
  }
  .provider-panel .footnote {
    margin-top: 18px;
  }
  .refresh-provider {
    margin-top: 16px;
  }
</style>
