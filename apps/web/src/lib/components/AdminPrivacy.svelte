<script lang="ts">
  import { onMount } from 'svelte';
  import { RefreshCw } from '@lucide/svelte';
  import Dialog from './Dialog.svelte';
  import LoadOlder from './LoadOlder.svelte';
  import { parsePrivacyQueue, privacyEvidence, type PrivacyRequest } from '#lib/privacy.ts';
  let requests = $state.raw<PrivacyRequest[]>([]);
  let cursor = $state<string | null>(null);
  let loading = $state(true);
  let busy = $state(false);
  let denied = $state(false);
  let error = $state('');
  let message = $state('');
  let selected = $state.raw<PrivacyRequest | null>(null);
  let decision = $state<'approve' | 'hold' | 'release' | 'complete'>('approve');
  let reason = $state('');
  let storage = $state('');
  let backups = $state('');
  let providers = $state('');
  let dialogError = $state('');
  let controller: AbortController | undefined;
  let generation = 0;
  let disposed = false;
  const names = {
    approve: 'Approve erasure processing',
    hold: 'Place legal hold',
    release: 'Release legal hold',
    complete: 'Record external completion evidence',
  };
  function clearDialog() {
    selected = null;
    reason = '';
    storage = '';
    backups = '';
    providers = '';
    dialogError = '';
  }
  function isolate() {
    denied = true;
    requests = [];
    cursor = null;
    clearDialog();
  }
  onMount(() => {
    void load();
    return () => {
      disposed = true;
      generation++;
      controller?.abort();
      clearDialog();
      requests = [];
    };
  });
  async function load(older = false) {
    if (busy || (older && loading)) return;
    const next = older ? cursor : null;
    if (older && !next) return;
    controller?.abort();
    const request = new AbortController();
    controller = request;
    const token = ++generation;
    loading = true;
    error = '';
    try {
      const response = await fetch(
        `/api/platform/admin/privacy${next ? `?cursor=${encodeURIComponent(next)}` : ''}`,
        { signal: request.signal, cache: 'no-store' },
      );
      const result = await response.json();
      if (disposed || request.signal.aborted || token !== generation) return;
      if (response.status === 401 || response.status === 403) isolate();
      if (!response.ok) throw new Error(result.message ?? 'Privacy requests could not be loaded.');
      const value = parsePrivacyQueue(result);
      denied = false;
      requests = older
        ? [
            ...requests,
            ...value.requests.filter((row) => !requests.some((previous) => previous.id === row.id)),
          ]
        : value.requests;
      cursor = value.next_cursor;
    } catch (e) {
      if (!disposed && !request.signal.aborted && token === generation)
        error = e instanceof Error ? e.message : 'Privacy requests could not be loaded.';
    } finally {
      if (!disposed && token === generation) {
        loading = false;
        controller = undefined;
      }
    }
  }
  function review(row: PrivacyRequest, action: typeof decision) {
    clearDialog();
    selected = row;
    decision = action;
  }
  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (
      !selected ||
      busy ||
      loading ||
      reason.trim().length < 5 ||
      (decision === 'complete' && ![storage, backups, providers].every(privacyEvidence))
    )
      return;
    const row = selected;
    const action = decision;
    const request = new AbortController();
    controller = request;
    busy = true;
    dialogError = '';
    message = '';
    try {
      const response = await fetch(
        `/api/platform/admin/privacy/${row.id}/${action === 'complete' ? 'complete' : 'review'}`,
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(
            action === 'complete'
              ? {
                  reason: reason.trim(),
                  storage_evidence: storage,
                  backup_evidence: backups,
                  provider_evidence: providers,
                }
              : { decision: action, reason: reason.trim() },
          ),
          signal: request.signal,
        },
      );
      const result = response.status === 204 ? null : await response.json();
      if (disposed || request.signal.aborted) return;
      if (response.status === 401 || response.status === 403) {
        isolate();
        error = result?.message ?? 'Sign in with recent MFA to review privacy requests.';
        return;
      }
      if (!response.ok)
        throw new Error(result?.message ?? 'The privacy decision could not be recorded.');
      clearDialog();
      message = 'Decision recorded. Refreshing the server-confirmed request status.';
    } catch (e) {
      if (!disposed && !request.signal.aborted)
        dialogError =
          e instanceof Error ? e.message : 'The privacy decision could not be recorded.';
    } finally {
      if (!disposed) {
        busy = false;
        controller = undefined;
      }
    }
    if (!disposed && !selected && !denied) await load();
  }
</script>

<div class="privacy-heading">
  <div>
    <h2>Privacy requests</h2>
    <p class="section-description">
      Application erasure, historical storage versions, and externally verified completion are
      distinct stages. Holds and financial obligations stop processing; retained evidence follows
      the configured retention policy.
    </p>
  </div>
  <button
    class="icon-button"
    aria-label="Refresh privacy requests"
    onclick={() => load()}
    disabled={busy || loading}><RefreshCw size={18} /></button
  >
</div>
{#if error}<p class="error-message" role="alert">{error}</p>{/if}{#if message}<p
    class="status-message"
    role="status"
  >
    {message}
  </p>{/if}
{#if denied}<a class="button-secondary" href="/auth">Sign in with recent MFA</a>{:else}
  {#if loading && !requests.length}<p role="status">
      Loading privacy requests…
    </p>{:else if !requests.length}<p class="section-description">
      No privacy requests are waiting in this queue.
    </p>{/if}
  {#each requests as row (row.id)}<article class="privacy-row">
      <div class="list-row">
        <div>
          <strong>{row.display_name}</strong><small class="reference">{row.id}</small><small
            >Requested {new Date(row.created_at).toLocaleString()} · Earliest processing {new Date(
              row.not_before,
            ).toLocaleString()}</small
          >
        </div>
        <span class="status-pill">{row.status.replaceAll('_', ' ')}</span>{#if row.legal_hold}<span
            class="status-pill">Legal hold</span
          >{/if}
      </div>
      <p class="footnote">
        {row.has_obligations
          ? 'Financial or contractual obligations prevent erasure.'
          : 'No blocking obligations reported.'} · {row.objects_pending} storage objects pending.
      </p>
      <dl class="retention">
        <dt>Request delay</dt>
        <dd>{row.retention.erasure_delay_days} days</dd>
        <dt>Backups</dt>
        <dd>{row.retention.backup_days} days</dd>
        <dt>Security</dt>
        <dd>{row.retention.security_days} days</dd>
        <dt>Financial evidence</dt>
        <dd>{row.retention.financial_days} days</dd>
        <dt>Policy evidence</dt>
        <dd>{row.retention.policy_evidence_days} days</dd>
        <dt>Moderation evidence</dt>
        <dd>{row.retention.moderation_evidence_days} days</dd>
      </dl>
      <p class="footnote">
        Application erasure: {row.application_erased_at
          ? new Date(row.application_erased_at).toLocaleString()
          : 'Not recorded'}. Historical version erasure: {row.versions_erased_at
          ? new Date(row.versions_erased_at).toLocaleString()
          : 'Not recorded'}. External completion: {row.completed_at
          ? new Date(row.completed_at).toLocaleString()
          : 'Not recorded'}.
      </p>
      {#if row.latest_action}<details>
          <summary>Latest recorded decision</summary>
          <p>
            {row.latest_action.action} · {new Date(row.latest_action.created_at).toLocaleString()}
          </p>
          <p>{row.latest_action.reason}</p>
          {#each Object.entries(row.latest_action.evidence) as [kind, reference] (kind)}<p
              class="reference"
            >
              {kind}: {reference}
            </p>{/each}
        </details>{/if}
      {#if row.status !== 'completed'}<div class="form-actions">
          {#if row.status === 'pending'}<button
              class="button-secondary"
              disabled={busy || loading || row.has_obligations || row.legal_hold}
              onclick={() => review(row, 'approve')}>Approve processing</button
            >{/if}<button
            class="button-secondary"
            disabled={busy || loading}
            onclick={() => review(row, row.legal_hold ? 'release' : 'hold')}
            >{row.legal_hold ? 'Release hold' : 'Place hold'}</button
          >{#if row.status === 'application_erased'}<button
              class="button-secondary"
              disabled={busy ||
                loading ||
                row.has_obligations ||
                row.legal_hold ||
                !row.versions_erased_at}
              onclick={() => review(row, 'complete')}>Record external completion</button
            >{/if}
        </div>{/if}
    </article>{/each}
  <LoadOlder {cursor} {loading} onload={() => load(true)} label="Older privacy requests" />
{/if}
{#if selected}<Dialog
    title={names[decision]}
    onclose={() => {
      if (!busy) clearDialog();
    }}
    ><form class="form-grid" onsubmit={submit}>
      <p class="reference">Request {selected.id}</p>
      <p>
        {decision === 'approve'
          ? 'Authorize queued application erasure after the configured delay. Financial obligations and legal holds must be clear. Approval does not certify external deletion.'
          : decision === 'hold'
            ? 'Pause unfinished erasure processing under a documented legal hold. A bounded storage batch already in progress may finish before the hold takes effect.'
            : decision === 'release'
              ? 'Release the documented hold. Processing still requires financial and retention checks.'
              : 'Attest that the referenced storage, backup, and provider evidence has been reviewed. The server verifies historical version erasure, elapsed backup retention, holds, and obligations before recording completion.'}
      </p>
      {#if dialogError}<p class="error-message" role="alert">{dialogError}</p>{/if}<label
        >Privacy decision reason<textarea bind:value={reason} minlength="5" maxlength="500" required
        ></textarea></label
      >{#if decision === 'complete'}<p class="footnote">
          Enter reference IDs or hashes, not personal information or free-text evidence. Each
          reference is 8–200 characters using letters, numbers, hyphens, underscores, periods,
          colons, or slashes.
        </p>
        <label
          >Storage evidence reference<input
            bind:value={storage}
            minlength="8"
            maxlength="200"
            required
          /></label
        ><label
          >Backup evidence reference<input
            bind:value={backups}
            minlength="8"
            maxlength="200"
            required
          /></label
        ><label
          >Provider evidence reference<input
            bind:value={providers}
            minlength="8"
            maxlength="200"
            required
          /></label
        >{/if}
      <div class="form-actions">
        <button
          class="button-primary"
          disabled={busy ||
            reason.trim().length < 5 ||
            (decision === 'complete' && ![storage, backups, providers].every(privacyEvidence))}
          >{busy ? 'Recording…' : 'Confirm privacy decision'}</button
        ><button type="button" class="button-secondary" disabled={busy} onclick={clearDialog}
          >Cancel</button
        >
      </div>
    </form></Dialog
  >{/if}

<style>
  .privacy-heading {
    display: flex;
    gap: 16px;
    align-items: start;
    justify-content: space-between;
  }
  .privacy-row {
    padding: 20px 0;
    border-bottom: 1px solid var(--border);
    font-size: 12px;
  }
  .privacy-row .list-row {
    align-items: start;
    flex-wrap: wrap;
  }
  .privacy-row small {
    display: block;
    margin-top: 8px;
  }
  .reference {
    overflow-wrap: anywhere;
    font-family: monospace;
  }
  .retention {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 8px 20px;
    margin: 16px 0;
    font-size: 11px;
  }
  .retention dd {
    margin: 0;
    color: var(--muted);
  }
  .privacy-row .form-actions {
    margin-top: 16px;
  }
  .privacy-row details {
    margin: 14px 0;
    line-height: 1.8;
  }
  .privacy-row summary {
    cursor: pointer;
    font-weight: 600;
  }
  .privacy-row details p {
    margin-top: 8px;
  }
</style>
