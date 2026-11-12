<script lang="ts">
  import CollectionPages from './CollectionPages.svelte';
  interface Payment {
    id: string;
    checkout_id: string;
    creator_id: string;
    creator_name: string;
    kind: 'subscription' | 'purchase' | 'tip';
    status: 'pending' | 'paid' | 'partially_refunded' | 'refunded' | 'failed';
    amount_minor: number;
    refunded_minor: number;
    currency: 'USD';
    created_at: string;
    period_start: string | null;
    period_end: string | null;
    provider: 'sandbox' | 'stripe';
    dispute_recorded: boolean;
  }
  let {
    payments = [],
    nextCursor = null,
    failed = false,
  }: {
    payments?: Payment[];
    nextCursor?: string | null;
    failed?: boolean;
  } = $props();
  const money = (amount: number, currency: string) =>
    new Intl.NumberFormat('en-US', { style: 'currency', currency }).format(amount / 100);
  function date(value: string | null) {
    if (!value) return null;
    const parsed = new Date(value);
    return Number.isFinite(parsed.getTime()) ? parsed.toLocaleDateString() : null;
  }
</script>

<section class="panel payment-history" aria-label="Payment history">
  <h2>Payments and refunds</h2>
  <p class="section-description">
    Recorded charges, recurring subscription periods, and refunds for your account. Recorded payment
    status does not guarantee current content access or bank settlement.
  </p>
  {#if failed}<p>Payment records could not be loaded. Refresh the page to try again.</p>
  {:else if !payments.length}<p>No payment records on this page.</p>{/if}
  {#each payments as payment (payment.id)}
    <article class="payment-record">
      <div class="payment-heading">
        <h3>{payment.creator_name}</h3>
        <span class="status-pill">{payment.status.replaceAll('_', ' ')}</span>
      </div>
      <p>
        {payment.kind === 'subscription'
          ? 'Subscription charge'
          : payment.kind === 'purchase'
            ? 'Content purchase'
            : 'Content-related tip'} · {money(payment.amount_minor, payment.currency)}
      </p>
      <p class="footnote">
        Recorded {date(payment.created_at) ?? 'date unavailable'} · {payment.provider === 'sandbox'
          ? 'Sandbox simulation; no real charge'
          : 'Stripe payment record'}
      </p>
      {#if payment.kind === 'subscription'}<p class="footnote">
          {#if date(payment.period_start) && date(payment.period_end)}Recorded subscription period: {date(
              payment.period_start,
            )} – {date(payment.period_end)}{:else}No subscription period is recorded on this
            payment.{/if}
        </p>{/if}
      {#if payment.refunded_minor > 0}<p>
          Refund recorded: {money(payment.refunded_minor, payment.currency)}
        </p>{/if}
      {#if payment.status === 'pending'}<p class="footnote">
          Payment confirmation is pending. Check this record or contact support before attempting
          another purchase.
        </p>{/if}
      {#if payment.dispute_recorded}<p class="footnote">
          A dispute has been recorded. This does not describe its current outcome or content access.
        </p>{/if}
      <p class="receipt-reference">Receipt reference: <code>{payment.id}</code></p>
    </article>
  {/each}
  <p class="footnote">
    For billing or refund help, use the <a href="/legal#refunds"
      >refund policy and support contact</a
    >. Never send card numbers, passwords, or recovery codes.
  </p>
  <CollectionPages
    {nextCursor}
    parameter="payments_cursor"
    label="Payment history"
    nextLabel="Older payment records"
  />
</section>

<style>
  .payment-record {
    border-top: 1px solid var(--border);
    padding: 18px 0;
  }
  .payment-heading {
    display: flex;
    gap: 10px;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
  }
  h3 {
    font-size: 13px;
  }
  .payment-record p {
    margin: 8px 0;
    font-size: 12px;
  }
  .receipt-reference {
    overflow-wrap: anywhere;
  }
  .receipt-reference code {
    font-size: 11px;
  }
  a {
    text-decoration: underline;
  }
</style>
