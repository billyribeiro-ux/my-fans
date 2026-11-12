<script lang="ts">
  import { onMount } from 'svelte';
  import { ShieldCheck, LockKeyhole, Check, ArrowUpRight } from '@lucide/svelte';
  import {
    hostedUrl,
    parseCheckout,
    parseIntegrationStatus,
    type IntegrationStatus,
    type CheckoutSession,
  } from '#lib/integrations.ts';
  import Dialog from './Dialog.svelte';
  let {
    creatorId,
    creatorName,
    amountMinor,
    kind,
    postId,
    onclose,
    oncomplete,
  }: {
    creatorId: string;
    creatorName: string;
    amountMinor: number;
    kind: 'subscription' | 'purchase' | 'tip';
    postId?: string;
    onclose: () => void;
    oncomplete: () => void;
  } = $props();
  let checkout = $state.raw<CheckoutSession | null>(null);
  let providers = $state.raw<IntegrationStatus | null>(null);
  let providerLoading = $state(true);
  let redirectUrl = $state('');
  let busy = $state(false);
  let error = $state('');
  let paid = $state(false);
  let key = '';
  let disposed = false;
  const controller = new AbortController();
  onMount(() => {
    void loadProviders();
    return () => {
      disposed = true;
      controller.abort();
      redirectUrl = '';
    };
  });
  async function loadProviders() {
    if (busy) return;
    providerLoading = true;
    error = '';
    try {
      const response = await fetch('/api/platform/integrations/status', {
        signal: controller.signal,
      });
      const result = await response.json();
      if (!response.ok) throw new Error(result.message ?? 'Unable to check the payment provider.');
      if (!disposed) providers = parseIntegrationStatus(result);
    } catch (e) {
      if (!disposed)
        error = e instanceof Error ? e.message : 'Unable to check the payment provider.';
    } finally {
      if (!disposed) providerLoading = false;
    }
  }
  const money = (amount: number, currency = 'USD') =>
    new Intl.NumberFormat('en-US', { style: 'currency', currency }).format(amount / 100);
  async function submit() {
    if (
      busy ||
      providerLoading ||
      !providers ||
      (providers.payments === 'stripe' && !providers.configured.payments)
    )
      return;
    if (redirectUrl) {
      const checked = hostedUrl(redirectUrl, 'checkout');
      redirectUrl = '';
      window.location.assign(checked);
      return;
    }
    if (checkout?.mode === 'stripe') {
      error = 'Reopen checkout to request a fresh Stripe link.';
      return;
    }
    busy = true;
    error = '';
    try {
      if (!key) key = crypto.randomUUID();
      const path = checkout ? `checkout/${checkout.id}/confirm` : 'checkout';
      const body = checkout
        ? {}
        : {
            kind,
            creator_id: creatorId,
            post_id: postId,
            amount_minor: amountMinor,
            idempotency_key: key,
          };
      const response = await fetch(`/api/platform/${path}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
        signal: controller.signal,
      });
      const result = await response.json();
      if (!response.ok)
        throw new Error(result.message ?? result.error ?? 'Unable to complete checkout.');
      if (disposed) return;
      if (checkout) {
        if (checkout.mode !== 'sandbox' || result.status !== 'paid' || result.mode !== 'sandbox')
          throw new Error(
            'Payment confirmation could not be verified. Please refresh your account.',
          );
        paid = true;
        oncomplete();
      } else {
        const checked = parseCheckout(result, providers.payments);
        checkout = checked.checkout;
        redirectUrl = checked.redirectUrl ?? '';
      }
    } catch (e) {
      if (!disposed) error = e instanceof Error ? e.message : 'Unable to complete checkout.';
    } finally {
      if (!disposed) busy = false;
    }
  }
</script>

<Dialog
  title={paid
    ? 'You’re all set'
    : checkout?.mode === 'stripe'
      ? 'Continue to Stripe Checkout'
      : checkout
        ? 'Confirm sandbox checkout'
        : 'Support what you love'}
  onclose={() => {
    if (!busy) {
      redirectUrl = '';
      onclose();
    }
  }}
>
  <div class="checkout-detail">
    <strong>{creatorName}</strong><span
      >{money(checkout?.amount_minor ?? amountMinor, checkout?.currency)}{kind === 'subscription'
        ? '/month'
        : ''}</span
    >
  </div>
  {#if providers}<div class="sandbox-notice">
      <ShieldCheck size={23} />
      <div>
        <strong>{providers.payments === 'sandbox' ? 'Sandbox payment' : 'Stripe payment'}</strong>
        <p>
          {providers.payments === 'sandbox'
            ? 'No real money is charged. No card details are collected. Transactions, access, and creator balances are simulated in the sandbox.'
            : 'Stripe collects your payment details on checkout.stripe.com. Access is updated only after the server confirms payment. Returning to MyFans alone does not confirm payment.'}
        </p>
      </div>
    </div>{:else}<p class="footnote" role="status">
      {providerLoading ? 'Checking payment provider…' : 'Payment provider is unavailable.'}
    </p>{/if}
  {#if providers?.payments === 'stripe' && !providers.configured.payments}<p
      class="error-message"
      role="alert"
    >
      Checkout is temporarily unavailable. Please try again later.
    </p>{/if}
  {#if error}<p class="error-message" role="alert">{error}</p>{/if}
  {#if paid}<div class="status-message" role="status">
      <Check size={17} /> Sandbox {kind === 'subscription'
        ? 'subscription'
        : kind === 'purchase'
          ? 'purchase'
          : 'tip'} confirmed.
    </div>
    <button class="button-primary full-width" onclick={onclose}>Back to your space</button>{:else}<p
      class="footnote"
    >
      {!providers
        ? 'Review the payment provider and terms before continuing.'
        : kind === 'subscription'
          ? providers?.payments === 'stripe'
            ? 'Review the subscription price and renewal terms on Stripe before paying.'
            : 'Access lasts for the stated subscription period. Sandbox billing does not charge a card or renew automatically.'
          : kind === 'purchase'
            ? 'This purchase unlocks the selected post for this account.'
            : 'This tip supports the creator and does not unlock paid posts.'}
    </p>
    {#if !providers && !providerLoading}<button
        class="button-secondary full-width"
        onclick={loadProviders}>Retry payment provider</button
      >{:else}<button
        class="button-primary full-width"
        onclick={submit}
        disabled={busy ||
          providerLoading ||
          !providers ||
          (providers.payments === 'stripe' && !providers.configured.payments)}
        ><LockKeyhole size={16} />{busy
          ? 'Processing…'
          : providerLoading
            ? 'Checking provider…'
            : redirectUrl
              ? `Continue to Stripe · ${money(checkout?.amount_minor ?? amountMinor, checkout?.currency)}`
              : checkout
                ? `Confirm simulated payment · ${money(checkout.amount_minor, checkout.currency)}`
                : providers?.payments === 'stripe'
                  ? 'Review Stripe checkout'
                  : 'Review sandbox checkout'}{#if redirectUrl}<ArrowUpRight
            size={16}
          />{/if}</button
      >{/if}{/if}
</Dialog>

<style>
  .checkout-detail {
    display: flex;
    justify-content: space-between;
    gap: 20px;
    margin-bottom: 22px;
    font-size: 16px;
  }
  .sandbox-notice {
    display: flex;
    gap: 13px;
    background: #edf2e5;
    padding: 18px;
    border-radius: 10px;
    margin-bottom: 18px;
  }
  .sandbox-notice strong {
    font-size: 13px;
  }
  .sandbox-notice p {
    font-size: 12px;
    line-height: 1.8;
    margin-top: 5px;
    color: var(--muted);
  }
  .footnote {
    margin-bottom: 20px;
  }
</style>
