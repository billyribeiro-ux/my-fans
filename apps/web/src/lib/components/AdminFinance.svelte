<script lang="ts">
  import { onMount } from 'svelte';
  import { refreshAll } from '$app/navigation';
  import { RefreshCw, ShieldAlert } from '@lucide/svelte';
  import Dialog from './Dialog.svelte';
  import LoadOlder from './LoadOlder.svelte';
  import {
    parseAdminFinance,
    type AdminFinance as FinanceData,
    type FinanceCollection,
  } from '#lib/finance.ts';
  type Action =
    | { kind: 'issue'; id: string }
    | { kind: 'checkout'; id: string; session: string }
    | { kind: 'dispute'; account: string; dispute: string };
  let finance = $state.raw<FinanceData | null>(null);
  let loading = $state(false);
  let busy = $state(false);
  let error = $state('');
  let message = $state('');
  let requiresSignIn = $state(false);
  let action = $state.raw<Action | null>(null);
  let reason = $state('');
  let sessionId = $state('');
  let restoreAccess = $state(false);
  let disposed = false;
  let controller: AbortController | undefined;
  onMount(() => {
    void load();
    return () => {
      disposed = true;
      controller?.abort();
      action = null;
      sessionId = '';
      reason = '';
      finance = null;
    };
  });
  async function request(path = '', body?: object, signal?: AbortSignal) {
    const response = await fetch(
      `/api/platform/admin/finance${path}`,
      body
        ? {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(body),
            signal,
          }
        : { signal },
    );
    const result = await response.json();
    if (disposed || signal?.aborted) throw new Error('Request cancelled.');
    if (!response.ok) {
      if (response.status === 401 || response.status === 403) {
        requiresSignIn = true;
        finance = null;
        action = null;
        sessionId = '';
        reason = '';
      }
      throw new Error(result.message ?? 'Financial reconciliation is unavailable.');
    }
    requiresSignIn = false;
    return result;
  }
  async function load(collection?: FinanceCollection, cursor?: string) {
    if (busy || (loading && cursor)) return;
    controller?.abort();
    const current = new AbortController();
    controller = current;
    loading = true;
    error = '';
    try {
      const result = parseAdminFinance(
        await request(
          cursor && collection ? `?${collection}_cursor=${encodeURIComponent(cursor)}` : '',
          undefined,
          current.signal,
        ),
      );
      if (disposed || current.signal.aborted) return;
      if (cursor && collection && finance) {
        const merged = [
          ...new Map(
            [...finance[collection], ...result[collection]].map((row) => [row.id, row]),
          ).values(),
        ];
        finance = {
          ...finance,
          [collection]: merged,
          scanner: result.scanner,
          next_cursors: { ...finance.next_cursors, [collection]: result.next_cursors[collection] },
        };
      } else finance = result;
    } catch (e) {
      if (!disposed && !current.signal.aborted)
        error = e instanceof Error ? e.message : 'Financial reconciliation is unavailable.';
    } finally {
      if (!disposed && controller === current) loading = false;
    }
  }
  function confirm(next: Action) {
    action = next;
    reason = '';
    sessionId = next.kind === 'checkout' ? next.session : '';
    restoreAccess = false;
    error = '';
  }
  function close() {
    if (!busy) {
      action = null;
      reason = '';
      sessionId = '';
      restoreAccess = false;
    }
  }
  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy || !action || requiresSignIn || reason.trim().length < 5) return;
    const selected = action;
    const current = new AbortController();
    controller?.abort();
    controller = current;
    busy = true;
    error = '';
    message = '';
    try {
      if (selected.kind === 'issue') {
        const result = await request(
          '/issues/acknowledge',
          { issue_id: selected.id, reason: reason.trim() },
          current.signal,
        );
        if (result.status !== 'acknowledged' || result.issue_id !== selected.id)
          throw new Error('Acknowledgment could not be verified. Refresh reconciliation data.');
        message =
          'Issue acknowledged. It remains unresolved until the reconciliation scanner verifies the financial state.';
      } else if (selected.kind === 'checkout') {
        const result = await request(
          '/checkout',
          { checkout_id: selected.id, session_id: sessionId.trim(), reason: reason.trim() },
          current.signal,
        );
        if (
          result.checkout_id !== selected.id ||
          !['pending', 'paid', 'failed'].includes(result.status)
        )
          throw new Error(
            'Checkout reconciliation could not be verified. Refresh reconciliation data.',
          );
        message = `Server-confirmed checkout status: ${result.status}.`;
      } else {
        const result = await request(
          '/dispute',
          {
            account_id: selected.account,
            dispute_id: selected.dispute,
            restore_access: restoreAccess,
            reason: reason.trim(),
          },
          current.signal,
        );
        if (typeof result.status !== 'string' || typeof result.restored !== 'boolean')
          throw new Error(
            'Dispute reconciliation could not be verified. Refresh reconciliation data.',
          );
        message = `Server-confirmed dispute status: ${result.status}. ${result.restored ? 'Eligible access was restored by the server.' : 'No access restoration was confirmed.'}`;
      }
      action = null;
      sessionId = '';
      reason = '';
      restoreAccess = false;
      await refreshAll();
    } catch (e) {
      if (!disposed && !current.signal.aborted)
        error = e instanceof Error ? e.message : 'Unable to reconcile this record.';
    } finally {
      if (!disposed) {
        busy = false;
        loading = false;
      }
    }
    if (!disposed && !requiresSignIn) await load();
  }
  const date = (value: string | null) =>
    value ? new Date(value).toLocaleString() : 'Not recorded';
  const money = (amount: number, currency: string) =>
    new Intl.NumberFormat('en-US', { style: 'currency', currency: currency.toUpperCase() }).format(
      amount / 100,
    );
</script>

<div class="finance-heading">
  <div>
    <h2>Financial reconciliation</h2>
    <p class="section-description">
      Provider records, reconciliation queues, and audited remediation. Server confirmation governs
      every change.
    </p>
  </div>
  <button class="button-secondary" onclick={() => load()} disabled={loading || busy}
    ><RefreshCw size={16} />{loading ? 'Refreshing…' : 'Refresh financial status'}</button
  >
</div>
{#if error}<p class="error-message" role="alert">{error}</p>{/if}{#if message}<p
    class="status-message"
    role="status"
  >
    {message}
  </p>{/if}
{#if requiresSignIn}<div class="review-notice">
    <ShieldAlert size={20} />
    <p>
      Recent administrator MFA is required. <a class="text-button" href="/auth"
        >Sign in again with your authenticator</a
      >, or <a class="text-button" href="/settings">complete authenticator recovery</a>.
    </p>
  </div>{/if}
{#if finance}
  <p class="footnote">
    {finance.mode === 'sandbox'
      ? 'Sandbox financial state. No real funds are reconciled or transferred.'
      : 'Stripe financial state. Reconciliation retrieves provider records; acknowledgment does not change money or resolve an issue.'}
  </p>
  <dl class="finance-counters">
    <div>
      <dt>Open issues</dt>
      <dd>{finance.scanner.open_issues}</dd>
    </div>
    <div>
      <dt>Failed jobs</dt>
      <dd>{finance.scanner.failed_jobs}</dd>
    </div>
    <div>
      <dt>Unverified receipts</dt>
      <dd>{finance.scanner.unverified_receipts}</dd>
    </div>
    <div>
      <dt>Next scan</dt>
      <dd>{date(finance.scanner.next_at)}</dd>
    </div>
  </dl>
  <section class="finance-section">
    <h3>Unresolved issues</h3>
    <p class="section-description">
      Acknowledging records operator review. The scanner must verify resolution.
    </p>
    {#if !finance.issues.length}<p class="footnote">No unresolved issues on this page.</p>{/if}
    {#each finance.issues as item (item.id)}<article class="finance-record">
        <strong>{item.code}</strong>
        <p>Account: {item.account_id} · Object: {item.object_id}</p>
        <p>First seen: {date(item.first_seen_at)} · Last seen: {date(item.last_seen_at)}</p>
        {#if item.acknowledged_at}<span class="status-pill"
            >Acknowledged · {date(item.acknowledged_at)}</span
          >{:else}<button
            class="button-secondary"
            disabled={busy || loading || requiresSignIn}
            onclick={() => confirm({ kind: 'issue', id: item.id })}>Acknowledge issue</button
          >{/if}
      </article>{/each}
    <LoadOlder
      cursor={finance.next_cursors.issues}
      loading={loading || busy}
      label="Older unresolved issues"
      onload={(cursor) => load('issues', cursor)}
    />
  </section>
  <section class="finance-section">
    <h3>Pending checkouts</h3>
    <p class="section-description">
      Link a provider session only when its exact checkout and connected account can be verified.
    </p>
    {#if !finance.checkouts.length}<p class="footnote">
        No stale pending checkouts on this page.
      </p>{/if}
    {#each finance.checkouts as item (item.id)}<article class="finance-record">
        <strong>Checkout {item.id}</strong>
        <p>Account: {item.provider_account}</p>
        <p>
          Provider session: {item.provider_session ?? 'Not linked'} · Created: {date(
            item.created_at,
          )}
        </p>
        <button
          class="button-secondary"
          disabled={busy || loading || requiresSignIn || finance.mode !== 'stripe'}
          onclick={() =>
            confirm({ kind: 'checkout', id: item.id, session: item.provider_session ?? '' })}
          >Reconcile checkout</button
        >
      </article>{/each}
    <LoadOlder
      cursor={finance.next_cursors.checkouts}
      loading={loading || busy}
      label="Older pending checkouts"
      onload={(cursor) => load('checkouts', cursor)}
    />
  </section>
  <section class="finance-section">
    <h3>Disputes</h3>
    <p class="section-description">
      Sync the current provider decision. Access restoration is allowed only when the server
      verifies all eligibility conditions.
    </p>
    {#if !finance.disputes.length}<p class="footnote">No dispute records on this page.</p>{/if}
    {#each finance.disputes as item (item.id)}<article class="finance-record">
        <strong>{item.dispute_id} · {money(item.amount_minor, item.currency)}</strong>
        <p>Account: {item.account_id} · Charge: {item.charge_id}</p>
        <p>Response due: {date(item.due_by)} · Checked: {date(item.checked_at)}</p>
        <span class="status-pill">{item.status}</span><button
          class="button-secondary"
          disabled={busy || loading || requiresSignIn || finance.mode !== 'stripe'}
          onclick={() =>
            confirm({ kind: 'dispute', account: item.account_id, dispute: item.dispute_id })}
          >Review dispute reconciliation</button
        >
      </article>{/each}
    <LoadOlder
      cursor={finance.next_cursors.disputes}
      loading={loading || busy}
      label="Older disputes"
      onload={(cursor) => load('disputes', cursor)}
    />
  </section>
  <section class="finance-section">
    <h3>Provider payouts</h3>
    <p class="section-description">
      Status comes from the provider. Reconciled membership describes the underlying balance
      records.
    </p>
    {#if !finance.payouts.length}<p class="footnote">
        No provider payout records on this page.
      </p>{/if}
    {#each finance.payouts as item (item.id)}<article class="finance-record">
        <strong>{item.payout_id} · {money(item.amount_minor, item.currency)}</strong>
        <p>
          Account: {item.account_id} · {item.automatic ? 'Automatic' : 'Manual'} provider payout
        </p>
        <p>Arrival: {date(item.arrival_at)} · Checked: {date(item.checked_at)}</p>
        <span class="status-pill">{item.status}</span>
        <p>
          Reconciliation: {item.reconciliation_status} · Members: {item.members_complete
            ? 'complete'
            : 'not complete'}
        </p>
      </article>{/each}
    <LoadOlder
      cursor={finance.next_cursors.payouts}
      loading={loading || busy}
      label="Older provider payouts"
      onload={(cursor) => load('payouts', cursor)}
    />
  </section>
  <section class="finance-section">
    <h3>Account scan schedule</h3>
    {#if !finance.accounts.length}<p class="footnote">
        No scheduled account scans on this page.
      </p>{/if}
    {#each finance.accounts as item (item.id)}<article class="finance-record">
        <strong>{item.account_id} · {item.kind}</strong>
        <p>Last completed: {date(item.last_completed_at)} · Next: {date(item.next_at)}</p>
      </article>{/each}
    <LoadOlder
      cursor={finance.next_cursors.accounts}
      loading={loading || busy}
      label="Older account schedules"
      onload={(cursor) => load('accounts', cursor)}
    />
  </section>
{:else if loading}<p class="footnote" role="status">Loading financial reconciliation…</p>{/if}

{#if action}<Dialog
    title={action.kind === 'issue'
      ? 'Acknowledge unresolved issue'
      : action.kind === 'checkout'
        ? 'Reconcile provider checkout'
        : 'Reconcile provider dispute'}
    onclose={close}
  >
    <form class="form-grid" onsubmit={submit}>
      {#if action.kind === 'checkout'}<p class="section-description">
          The server retrieves the session from Stripe and verifies its account, checkout metadata,
          amount, and currency. A successful request may still return pending or failed.
        </p>
        <p class="footnote">Checkout: {action.id}</p>
        <label
          >Stripe session ID<input
            aria-label="Stripe session ID"
            bind:value={sessionId}
            required
            maxlength="255"
            autocomplete="off"
          /></label
        >
      {:else if action.kind === 'dispute'}<p class="section-description">
          Retrieve the current dispute decision. No refund is issued by this action.
        </p>
        <p class="footnote">Dispute: {action.dispute} · Account: {action.account}</p>
        <label class="restore-confirm"
          ><input type="checkbox" bind:checked={restoreAccess} /><span
            >Request access restoration if the provider confirms a merchant-won dispute and the
            server verifies no other blocking reason.</span
          ></label
        >
      {:else}<p class="section-description">
          This records acknowledgment only. The issue remains unresolved until reconciliation
          verifies the financial state.
        </p>
        <p class="footnote">Issue: {action.id}</p>{/if}
      <label
        >Reason for the audit record<textarea
          aria-label="Reason for the audit record"
          bind:value={reason}
          minlength="5"
          maxlength="500"
          required></textarea></label
      >
      {#if error}<p class="error-message" role="alert">{error}</p>{/if}
      <button
        class="button-primary"
        disabled={busy ||
          reason.trim().length < 5 ||
          (action.kind === 'checkout' && !sessionId.trim())}
        >{busy
          ? 'Checking server state…'
          : action.kind === 'issue'
            ? 'Confirm acknowledgment'
            : 'Confirm provider reconciliation'}</button
      >
    </form>
  </Dialog>{/if}

<style>
  .finance-heading {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 20px;
    flex-wrap: wrap;
    margin-top: 24px;
  }
  .finance-heading > div {
    flex: 1;
    min-width: 220px;
  }
  .finance-counters {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 15px;
    margin: 24px 0;
  }
  .finance-counters > div {
    background: #f0f2e9;
    padding: 16px;
    border-radius: 10px;
  }
  dt {
    font-size: 11px;
    color: var(--muted);
  }
  dd {
    font-size: 15px;
    font-weight: 600;
    margin-top: 8px;
    overflow-wrap: anywhere;
  }
  .finance-section {
    margin-top: 30px;
    padding-top: 24px;
    border-top: 1px solid var(--border);
  }
  .finance-section h3 {
    font-size: 17px;
  }
  .finance-record {
    padding: 20px 0;
    border-bottom: 1px solid var(--border);
    overflow-wrap: anywhere;
  }
  .finance-record strong {
    font-size: 13px;
  }
  .finance-record p {
    font-size: 12px;
    line-height: 1.8;
    color: var(--muted);
    margin: 6px 0;
  }
  .finance-record button {
    margin-top: 10px;
  }
  .restore-confirm {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    line-height: 1.8;
  }
  .restore-confirm input {
    width: auto;
    flex: 0 0 auto;
    margin-top: 4px;
  }
  @media (max-width: 760px) {
    .finance-counters {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
</style>
