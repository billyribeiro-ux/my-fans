<script lang="ts">
  import { onMount } from 'svelte';
  import { RefreshCw, ShieldAlert, RotateCcw } from '@lucide/svelte';
  import Dialog from './Dialog.svelte';
  import LoadOlder from './LoadOlder.svelte';

  interface Operations {
    jobs: {
      id: string;
      kind: string;
      status: string;
      attempts: number;
      run_at: string;
      last_error: string | null;
    }[];
    audit: {
      id: string;
      action: string;
      resource_type: string;
      resource_id: string | null;
      created_at: string;
    }[];
    users: {
      id: string;
      display_name: string;
      email: string;
      role: string;
      disabled_at: string | null;
    }[];
    ledger_imbalance_count: number;
    queued_mail_count: number;
    outbox_pending_count: number;
    mode: 'sandbox' | 'production' | 'stripe';
    next_cursors?: Record<'jobs' | 'audit' | 'users', string | null>;
  }
  type Confirmation =
    | { kind: 'account'; id: string; name: string; action: 'suspend' | 'restore' }
    | { kind: 'post'; id: string; name: string; action: 'restore' };
  let operations = $state.raw<Operations | null>(null);
  let loading = $state(false);
  let busyId = $state('');
  let error = $state('');
  let message = $state('');
  let requiresSignIn = $state(false);
  let confirmation = $state<Confirmation | null>(null);
  let reason = $state('');
  let activeJobs = $derived(operations?.jobs.filter((job) => job.status !== 'done').length ?? 0);

  let controller: AbortController | undefined;
  let disposed = false;
  const mutations = new AbortController();
  async function request(path: string, body?: object, signal?: AbortSignal) {
    if (disposed) throw new Error('Request cancelled.');
    const response = await fetch(
      `/api/platform/admin/${path}`,
      body
        ? {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(body),
            signal: signal ?? mutations.signal,
          }
        : { signal },
    );
    const result = await response.json();
    if (disposed || signal?.aborted) throw new Error('Request cancelled.');
    if (!response.ok) {
      requiresSignIn = response.status === 401 || response.status === 403;
      if (requiresSignIn) {
        operations = null;
        confirmation = null;
        reason = '';
      }
      throw new Error(result.message ?? result.error ?? 'The operation could not be completed.');
    }
    requiresSignIn = false;
    return result;
  }
  async function load(collection?: 'jobs' | 'audit' | 'users', cursor?: string) {
    if (disposed || (cursor && (loading || busyId))) return;
    controller?.abort();
    const current = new AbortController();
    controller = current;
    loading = true;
    error = '';
    try {
      const result = await request(
        `operations${cursor && collection ? `?${collection}_cursor=${encodeURIComponent(cursor)}` : ''}`,
        undefined,
        current.signal,
      );
      if (disposed || current.signal.aborted) return;
      if (cursor && collection && operations) {
        const existing = operations[collection];
        operations = {
          ...operations,
          [collection]: [
            ...existing,
            ...result[collection].filter(
              (item: { id: string }) => !existing.some((old) => old.id === item.id),
            ),
          ],
          next_cursors: {
            ...operations.next_cursors,
            [collection]: result.next_cursors?.[collection] ?? null,
          },
        } as Operations;
      } else operations = result;
    } catch (e) {
      if (!disposed && !current.signal.aborted)
        error = e instanceof Error ? e.message : 'Operations could not be loaded.';
    } finally {
      if (!disposed && !current.signal.aborted) loading = false;
    }
  }
  onMount(() => {
    void load();
    return () => {
      disposed = true;
      controller?.abort();
      mutations.abort();
      operations = null;
      confirmation = null;
      reason = '';
    };
  });

  function confirm(value: Confirmation) {
    confirmation = value;
    reason = '';
    error = '';
    message = '';
  }
  async function retry(id: string) {
    if (disposed || busyId) return;
    busyId = id;
    error = '';
    message = '';
    try {
      await request(`jobs/${id}/retry`, {});
      message = 'Job queued for another attempt. The retry has been audited.';
      await load();
    } catch (e) {
      if (!disposed) error = e instanceof Error ? e.message : 'Job could not be retried.';
    } finally {
      if (!disposed) busyId = '';
    }
  }
  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (disposed || busyId || !confirmation || reason.trim().length < 5) return;
    const action = confirmation;
    busyId = action.id;
    error = '';
    message = '';
    try {
      await request(
        action.kind === 'post' ? `posts/${action.id}/restore` : `accounts/${action.id}`,
        action.kind === 'post'
          ? { reason: reason.trim() }
          : { action: action.action, reason: reason.trim() },
      );
      message =
        action.kind === 'post'
          ? 'Moderation block cleared. The post remains a draft.'
          : action.action === 'suspend'
            ? 'Account suspended and existing sessions revoked.'
            : 'Account restored. Creator profiles require a new review; removed posts remain blocked.';
      confirmation = null;
      reason = '';
      await load();
    } catch (e) {
      if (!disposed) error = e instanceof Error ? e.message : 'Decision could not be saved.';
    } finally {
      if (!disposed) busyId = '';
    }
  }
</script>

<div class="operations">
  <div class="list-row">
    <div>
      <h2>Platform operations</h2>
      <p>Queue health, account safety, and the latest 100 audit events.</p>
    </div>
    <button class="button-secondary" onclick={() => load()} disabled={loading || !!busyId}
      ><RefreshCw size={16} />Refresh operations</button
    >
  </div>
  <p class="footnote">
    Sensitive administration requires an authenticator-verified sign-in from the last 15 minutes.
    Queue counters track work awaiting processing; they do not confirm recipient delivery or
    financial settlement.
  </p>
  {#if error}<p class="error-message" role="alert">
      {error}{#if requiresSignIn}
        <a href="/auth">Sign in again with your authenticator</a>.{/if}
    </p>{/if}
  {#if message}<p class="status-message" role="status">{message}</p>{/if}
  {#if loading && !operations}<p role="status">Loading operations…</p>{/if}
  {#if operations}
    <dl class="metrics">
      <div class={{ imbalanced: operations.ledger_imbalance_count > 0 }}>
        <dt>Ledger imbalances</dt>
        <dd>{operations.ledger_imbalance_count}</dd>
      </div>
      <div>
        <dt>Queued email deliveries</dt>
        <dd>{operations.queued_mail_count}</dd>
      </div>
      <div>
        <dt>Pending outbox events</dt>
        <dd>{operations.outbox_pending_count}</dd>
      </div>
      <div>
        <dt>Jobs loaded</dt>
        <dd>{activeJobs}</dd>
      </div>
    </dl>
    {#if operations.ledger_imbalance_count > 0}<p class="error-message" role="alert">
        <ShieldAlert size={18} /> Ledger integrity needs investigation before further financial operations.
      </p>{/if}
    {#if confirmation}
      <Dialog
        title={confirmation.kind === 'post'
          ? 'Restore post to drafts'
          : confirmation.action === 'suspend'
            ? 'Suspend account'
            : 'Restore account'}
        onclose={() => {
          if (!busyId) confirmation = null;
        }}
      >
        <form class="form-grid confirmation" onsubmit={submit}>
          {#if error}<p class="error-message" role="alert">
              {error}{#if requiresSignIn}
                <a href="/auth">Sign in again with your authenticator</a>.{/if}
            </p>{/if}
          <p><strong>{confirmation.name}</strong></p>
          <p>
            {confirmation.kind === 'post'
              ? 'Clear an existing moderation block for this audited post. It remains a draft and requires the creator to publish it. The server rejects posts without a moderation block.'
              : confirmation.action === 'suspend'
                ? 'Revoke all account sessions, block sign-in, and remove creator posts from public access. This decision is audited.'
                : 'Allow the account to sign in again. Creator approval returns to pending review. Removed posts remain blocked until individually reviewed.'}
          </p>
          <label
            >Operation reason<textarea
              bind:value={reason}
              required
              minlength="5"
              maxlength="2000"
              placeholder="Explain the reviewed evidence and decision…"></textarea></label
          >
          <div class="form-actions">
            <button class="button-primary" disabled={!!busyId || reason.trim().length < 5}
              >Confirm {confirmation.kind === 'post'
                ? 'post restoration'
                : confirmation.action === 'suspend'
                  ? 'suspension'
                  : 'account restoration'}</button
            ><button
              type="button"
              class="button-secondary"
              disabled={!!busyId}
              onclick={() => (confirmation = null)}>Cancel</button
            >
          </div>
        </form>
      </Dialog>
    {/if}
    <section aria-labelledby="jobs-heading">
      <h3 id="jobs-heading">Background jobs</h3>
      {#if !operations.jobs.length}<p class="footnote">No failed, pending, or running jobs.</p>{/if}
      {#each operations.jobs as job (job.id)}<article class="operation-row">
          <div class="list-row">
            <div>
              <strong>{job.kind}</strong><small>{job.id}</small>
              <p>{job.attempts} attempts · next run {new Date(job.run_at).toLocaleString()}</p>
            </div>
            <span class="status-pill">{job.status}</span>{#if job.status === 'failed'}<button
                class="button-secondary"
                disabled={!!busyId}
                onclick={() => retry(job.id)}><RotateCcw size={15} />Retry failed job</button
              >{/if}
          </div>
          {#if job.last_error}<p class="error-detail">{job.last_error}</p>{/if}
        </article>{/each}
      <LoadOlder
        cursor={operations.next_cursors?.jobs}
        loading={loading || !!busyId}
        label="Older jobs"
        onload={(cursor) => load('jobs', cursor)}
      />
    </section>
    <section aria-labelledby="accounts-heading">
      <h3 id="accounts-heading">Account moderation</h3>
      <p class="footnote">Recent non-administrator accounts. Every decision requires a reason.</p>
      {#each operations.users as user (user.id)}<article class="operation-row list-row">
          <div>
            <strong>{user.display_name}</strong><small>{user.email}</small><small
              >{user.role} · {user.disabled_at ? 'Suspended' : 'Active'}</small
            >
          </div>
          {#if user.role !== 'admin'}<button
              class="button-secondary"
              disabled={!!busyId}
              onclick={() =>
                confirm({
                  kind: 'account',
                  id: user.id,
                  name: user.display_name,
                  action: user.disabled_at ? 'restore' : 'suspend',
                })}>{user.disabled_at ? 'Restore account' : 'Suspend account'}</button
            >{/if}
        </article>{/each}
      {#if !operations.users.length}<p class="footnote">No accounts to review.</p>{/if}
      <LoadOlder
        cursor={operations.next_cursors?.users}
        loading={loading || !!busyId}
        label="Older accounts"
        onload={(cursor) => load('users', cursor)}
      />
    </section>
    <section aria-labelledby="audit-heading">
      <h3 id="audit-heading">Audit trail</h3>
      <p class="footnote">
        Post restoration is available only for a post referenced in this audit trail.
      </p>
      {#each operations.audit as entry (entry.id)}<article class="operation-row list-row">
          <div>
            <strong>{entry.action}</strong><small
              >{entry.resource_type} · {entry.resource_id ?? 'No resource identifier'}</small
            ><small>{new Date(entry.created_at).toLocaleString()}</small>
          </div>
          {#if entry.resource_type === 'post' && entry.resource_id}<button
              class="button-secondary"
              disabled={!!busyId}
              onclick={() =>
                confirm({
                  kind: 'post',
                  id: entry.resource_id!,
                  name: `Post ${entry.resource_id}`,
                  action: 'restore',
                })}>Review post restoration</button
            >{/if}
        </article>{/each}
      {#if !operations.audit.length}<p class="footnote">No audit events yet.</p>{/if}
      <LoadOlder
        cursor={operations.next_cursors?.audit}
        loading={loading || !!busyId}
        label="Older audit events"
        onload={(cursor) => load('audit', cursor)}
      />
    </section>
  {/if}
</div>

<style>
  .operations {
    padding: 4px 0;
  }
  .operations > .list-row {
    align-items: start;
  }
  h2 {
    font-size: 21px;
    margin: 0 0 8px;
  }
  h3 {
    font-size: 16px;
    margin: 0 0 12px;
  }
  p {
    font-size: 13px;
    line-height: 1.8;
  }
  small {
    display: block;
    font-size: 11px;
    color: var(--muted);
    margin-top: 5px;
    overflow-wrap: anywhere;
  }
  section {
    margin-top: 32px;
  }
  .metrics {
    display: grid;
    grid-template-columns: repeat(4, minmax(0, 1fr));
    gap: 12px;
    margin: 22px 0;
  }
  .metrics div {
    background: #f4f5ee;
    border-radius: 12px;
    padding: 17px;
  }
  .metrics dt {
    font-size: 11px;
    line-height: 1.6;
    color: var(--muted);
  }
  .metrics dd {
    font-size: 28px;
    margin: 7px 0 0;
    font-weight: 600;
  }
  .metrics .imbalanced {
    background: #fae8e3;
    color: #953d2e;
  }
  .operation-row {
    border-top: 1px solid var(--border);
    padding: 18px 0;
    overflow-wrap: anywhere;
  }
  .operation-row .list-row,
  .operation-row.list-row {
    align-items: start;
  }
  .operation-row .list-row > div,
  .operation-row.list-row > div {
    min-width: 0;
    flex: 1;
  }
  .operation-row p {
    margin: 6px 0 0;
  }
  .error-detail {
    padding: 10px;
    background: #fae8e3;
    color: #953d2e;
    border-radius: 8px;
    font-size: 12px;
    white-space: pre-wrap;
  }
  .confirmation {
    margin: 0;
  }
  .confirmation strong {
    overflow-wrap: anywhere;
  }
  @media (max-width: 700px) {
    .operations {
      padding: 4px 0;
    }
    .metrics {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
    .operations > .list-row,
    .operation-row.list-row,
    .operation-row .list-row {
      flex-wrap: wrap;
    }
    .operations > .list-row > div {
      width: 100%;
    }
    .operation-row .list-row > div,
    .operation-row.list-row > div {
      flex-basis: 100%;
    }
  }
</style>
