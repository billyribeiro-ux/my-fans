<script lang="ts">
  import { onMount } from 'svelte';
  import { goto, refreshAll } from '$app/navigation';
  import { page } from '$app/state';
  import { Wallet, ArrowUpRight, ShieldCheck } from '@lucide/svelte';
  import CollectionPages from '#lib/components/CollectionPages.svelte';
  import IntegrationSetup from '#lib/components/IntegrationSetup.svelte';
  import type { PageProps } from './$types';
  let { data }: PageProps = $props();
  let disposed = false;
  const controller = new AbortController();
  onMount(() => () => {
    disposed = true;
    controller.abort();
  });
  let earnings = $derived(data.earnings);
  let paymentMode = $derived(earnings?.mode ?? data.integrations?.payments ?? null);
  let stripe = $derived(paymentMode === 'stripe');
  class PayoutDraft {
    amount = $state<number | undefined>(0);
    key = '';
    readonly userId: string | null;
    constructor(userId: string | null) {
      this.userId = userId;
    }
  }
  let userId = $derived(data.user?.id ?? null);
  let draft = $derived(new PayoutDraft(userId));
  let busy = $state(false);
  let error = $state('');
  let message = $state('');
  const money = (minor: number, currency = 'USD') =>
    new Intl.NumberFormat('en-US', { style: 'currency', currency }).format(minor / 100);
  async function payout(event: SubmitEvent) {
    event.preventDefault();
    const current = draft;
    if (
      busy ||
      paymentMode !== 'sandbox' ||
      !earnings ||
      data.error ||
      current.amount === undefined
    )
      return;
    busy = true;
    error = '';
    message = '';
    try {
      if (!current.key) current.key = crypto.randomUUID();
      const response = await fetch('/api/platform/payouts', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          amount_minor: Math.round(current.amount * 100),
          idempotency_key: current.key,
        }),
        signal: controller.signal,
      });
      const result = await response.json();
      if (!response.ok)
        throw new Error(result.message ?? result.error ?? 'Unable to request payout.');
      if (disposed || draft !== current) return;
      current.amount = 0;
      current.key = '';
      message = 'Sandbox payout request submitted for review. No real funds will be transferred.';
      if (page.url.searchParams.has('payouts_cursor')) {
        const url = new URL(page.url.href);
        url.searchParams.delete('payouts_cursor');
        await goto(`${url.pathname}${url.search}`, { reset: false });
      }
      if (disposed || draft !== current) return;
      await refreshAll();
    } catch (e) {
      if (!disposed && draft === current)
        error = e instanceof Error ? e.message : 'Unable to request payout.';
    } finally {
      if (!disposed) busy = false;
    }
  }
</script>

<svelte:head><title>Earnings · MyFans</title></svelte:head>
<main id="main-content">
  <div class="page-heading">
    <h1>Good work. Growing support.</h1>
    <p>
      Your creator balances, ledger history, and {stripe
        ? 'Stripe payout management'
        : paymentMode === 'sandbox'
          ? 'sandbox payout requests'
          : 'payment provider status'}.
    </p>
  </div>
  {#if error || data.error}<p class="error-message" role="alert">
      {error || data.error}
    </p>{/if}{#if message}<p class="status-message" role="status">{message}</p>{/if}
  {#if !data.user}<section class="panel empty-panel">
      <Wallet size={32} />
      <h2>Your work has value.</h2>
      <p>Sign in to access your creator earnings.</p>
      <a class="button-primary" href="/auth">Sign in <ArrowUpRight size={16} /></a>
    </section>{:else if data.user.role === 'fan'}<section class="panel empty-panel">
      <Wallet size={32} />
      <h2>Start your creator story.</h2>
      <p>Set up a creator profile to manage earnings and payouts.</p>
      <a class="button-primary" href="/studio">Open creator studio</a>
    </section>{:else}<div class="balance-grid">
      {#each [{ label: 'Available', value: earnings?.available_minor ?? 0 }, { label: 'Pending', value: earnings?.pending_minor ?? 0 }, { label: 'Reserved', value: earnings?.reserved_minor ?? 0 }, { label: 'Total held balance', value: earnings?.total_minor ?? 0 }] as item (item.label)}<section
          class="panel balance-card"
        >
          <p>{item.label}</p>
          <strong>{money(item.value, earnings?.currency)}</strong>
        </section>{/each}
    </div>
    <div class="sandbox-banner">
      <ShieldCheck size={21} />
      <p>
        {paymentMode === null
          ? 'Payment mode is unavailable. Refresh provider status before managing payouts.'
          : stripe
            ? 'Stripe manages bank payouts and payout schedules. Local sandbox payout requests are unavailable in Stripe mode.'
            : 'All displayed balances and payouts are sandbox simulations. No real funds are collected or transferred.'}
      </p>
    </div>
    <div class="two-column">
      <section class="panel">
        {#if stripe}
          {#key data.user.id}<IntegrationSetup
              status={data.integrations}
              initialError={data.integrationError}
              allowSetup
              paymentsOnly
            />{/key}
        {:else if paymentMode === 'sandbox'}<h2>Request a payout</h2>
          <p class="section-description">
            Available funds are reserved while your request is reviewed. Sandbox approval simulates
            a payout.
          </p>
          <form class="form-grid" onsubmit={payout}>
            <label
              >Amount in USD<input
                type="number"
                min="1"
                max={(earnings?.available_minor ?? 0) / 100}
                step="0.01"
                bind:value={draft.amount}
                disabled={busy}
                required
              /><small>Available: {money(earnings?.available_minor ?? 0)}</small></label
            ><button
              class="button-primary"
              disabled={busy ||
                !draft.amount ||
                draft.amount < 1 ||
                Math.round(draft.amount * 100) > (earnings?.available_minor ?? 0)}
              >Request sandbox payout</button
            >
          </form>{:else}<IntegrationSetup
            status={data.integrations}
            initialError={data.integrationError}
            allowSetup
            paymentsOnly
          />{/if}
        <h3 class="subheading">Payout history</h3>
        {#if !earnings?.payouts?.length}<p class="footnote">
            No payout requests yet.
          </p>{/if}{#each earnings?.payouts ?? [] as item (item.id)}<div class="list-row">
            <span
              ><strong>{money(item.amount_minor)}</strong><small
                >{new Date(item.created_at).toLocaleString()}</small
              ></span
            ><span class="status-pill">{item.status}</span>
          </div>{/each}
        <CollectionPages
          nextCursor={earnings?.next_cursors?.payouts}
          parameter="payouts_cursor"
          label="Payouts"
          nextLabel="Older payouts"
        />
      </section>
      <section class="panel">
        <h2>Transaction history</h2>
        <p class="section-description">
          Subscriptions, paid unlocks, tips, refunds, and ledger adjustments.
        </p>
        {#if !earnings?.transactions?.length}<p class="footnote">
            Your first supporter is waiting to discover you.
          </p>{/if}{#each earnings?.transactions ?? [] as item (item.id)}<div class="list-row">
            <span
              ><strong>{item.kind}</strong><small
                >{new Date(item.created_at).toLocaleString()}</small
              ></span
            ><strong>{money(item.amount_minor, earnings?.currency ?? 'USD')}</strong>
          </div>{/each}
        <CollectionPages
          nextCursor={earnings?.next_cursors?.transactions}
          parameter="transactions_cursor"
          label="Transactions"
          nextLabel="Older transactions"
        />
      </section>
    </div>{/if}
</main>

<style>
  .balance-grid {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: 16px;
  }
  .balance-card strong {
    font-size: 27px;
    font-weight: 600;
    letter-spacing: -1px;
    display: block;
    margin-top: 12px;
  }
  .balance-card p {
    font-size: 11px;
  }
  .sandbox-banner {
    display: flex;
    align-items: center;
    gap: 13px;
    margin: 22px 0;
    padding: 17px 20px;
    background: #edf2e5;
    border-radius: 12px;
    font-size: 12px;
    line-height: 1.8;
    color: #5d7150;
  }
  .section-description {
    margin-bottom: 20px;
  }
  .subheading {
    font-size: 15px;
    margin-top: 28px;
  }
  @media (max-width: 1100px) {
    .balance-grid {
      grid-template-columns: repeat(2, 1fr);
    }
  }
  @media (max-width: 400px) {
    .balance-card {
      padding: 18px;
    }
    .balance-card strong {
      font-size: 23px;
    }
  }
</style>
