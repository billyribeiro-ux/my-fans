<script lang="ts">
  import { onMount } from 'svelte';
  import { refreshAll } from '$app/navigation';
  import { Shield, Check, X, RefreshCw } from '@lucide/svelte';
  import AdminOperations from '#lib/components/AdminOperations.svelte';
  import AdminAppeals from '#lib/components/AdminAppeals.svelte';
  import AdminFinance from '#lib/components/AdminFinance.svelte';
  import AdminPrivacy from '#lib/components/AdminPrivacy.svelte';
  import AdminPostInspection from '#lib/components/AdminPostInspection.svelte';
  import CollectionPages from '#lib/components/CollectionPages.svelte';
  import { page } from '$app/state';
  import { goto } from '$app/navigation';
  import type { PageProps } from './$types';
  import type { AdminReview as Review } from '#lib/route-data.ts';
  let { data }: PageProps = $props();
  let disposed = false;
  const controller = new AbortController();
  onMount(() => () => {
    disposed = true;
    controller.abort();
  });
  const queues = [
    'creator',
    'media',
    'report',
    'payout',
    'checkout',
    'operations',
    'appeals',
    'finance',
    'privacy',
  ] as const;
  type Queue = (typeof queues)[number];
  let tab = $derived(
    queues.includes(page.url.searchParams.get('queue') as Queue)
      ? (page.url.searchParams.get('queue') as Queue)
      : 'creator',
  );
  let collection = $derived<'creators' | 'reports' | 'payouts' | 'checkouts' | 'media'>(
    tab === 'creator'
      ? 'creators'
      : tab === 'report'
        ? 'reports'
        : tab === 'payout'
          ? 'payouts'
          : tab === 'checkout'
            ? 'checkouts'
            : 'media',
  );
  function selectQueue(next: Queue) {
    const url = new URL(page.url.href);
    url.searchParams.set('queue', next);
    void goto(`${url.pathname}${url.search}`, { reset: false });
  }
  class ReviewDraft {
    reason = $state('');
    selected = $state<string | null>(null);
    inspectedPost = $state<string | null>(null);
    readonly scope: string;
    constructor(scope: string) {
      this.scope = scope;
    }
  }
  let reviewScope = $derived(`${data.user?.id ?? 'guest'}:${tab}`);
  let draft = $derived(new ReviewDraft(reviewScope));
  let busy = $state(false);
  let error = $state('');
  let message = $state('');
  let items = $derived(
    data.reviews?.[
      tab === 'creator'
        ? 'creators'
        : tab === 'report'
          ? 'reports'
          : tab === 'payout'
            ? 'payouts'
            : tab === 'checkout'
              ? 'checkouts'
              : 'media'
    ] ?? [],
  );
  function sandboxPayment(item: Review) {
    return (
      item.provider === 'sandbox' || (!item.provider && data.integrations?.payments === 'sandbox')
    );
  }
  async function refund(id: string) {
    const current = draft;
    if (busy || current.reason.trim().length < 5) return;
    busy = true;
    error = '';
    message = '';
    try {
      const response = await fetch(`/api/platform/admin/refunds/${id}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ reason: current.reason.trim() }),
        signal: controller.signal,
      });
      const result = await response.json();
      if (!response.ok) throw new Error(result.message ?? 'Refund could not be saved.');
      if (disposed || draft !== current) return;
      current.selected = null;
      if (result.mode === 'stripe' && result.status === 'processing')
        message =
          'Stripe refund requested. Final funds and paid access status await server confirmation.';
      else if (result.mode === 'stripe' && result.status === 'refunded')
        message =
          'The server confirms that Stripe has fully refunded this payment. Refresh records to review access status.';
      else if (result.mode === 'sandbox' && result.status === 'refunded')
        message = 'Sandbox refund recorded. Associated paid access has been revoked.';
      else
        throw new Error(
          'Refund status could not be verified. Refresh financial records before retrying.',
        );
      current.reason = '';
      await refreshAll();
    } catch (e) {
      if (!disposed && draft === current)
        error = e instanceof Error ? e.message : 'Refund could not be saved.';
    } finally {
      if (!disposed) busy = false;
    }
  }
  async function review(id: string, decision: 'approve' | 'reject') {
    const current = draft;
    if (busy || !current.reason.trim()) return;
    busy = true;
    error = '';
    message = '';
    try {
      const response = await fetch(`/api/platform/admin/reviews/${id}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ kind: tab, decision, reason: current.reason }),
        signal: controller.signal,
      });
      const result = await response.json();
      if (!response.ok)
        throw new Error(result.message ?? result.error ?? 'Review could not be saved.');
      if (disposed || draft !== current) return;
      current.reason = '';
      current.selected = null;
      message = 'Review recorded in the audit trail.';
      await refreshAll();
    } catch (e) {
      if (!disposed && draft === current)
        error = e instanceof Error ? e.message : 'Review could not be saved.';
    } finally {
      if (!disposed) busy = false;
    }
  }
</script>

<svelte:head><title>Administration · MyFans</title></svelte:head>
<main id="main-content">
  <div class="page-heading">
    <h1>Care for the community.</h1>
    <p>Community reviews, financial reconciliation, and audited platform operations.</p>
  </div>
  {#if error || data.error}<p class="error-message" role="alert">
      {error || data.error}
    </p>{/if}{#if message}<p class="status-message" role="status">{message}</p>{/if}
  {#if data.user?.role !== 'admin'}<section class="panel empty-panel">
      <Shield size={32} />
      <h2>Administrator access required.</h2>
      <p>Review actions are restricted to authorized administrators.</p>
      <a class="button-primary" href={data.user ? '/' : '/auth'}
        >{data.user ? 'Back to your feed' : 'Sign in'}</a
      >
    </section>{:else if !data.user.mfa_enabled}<section class="panel empty-panel">
      <Shield size={32} />
      <h2>Secure your administrator account.</h2>
      <p>Enable authenticator verification before reviewing sensitive actions.</p>
      <a class="button-primary" href="/settings">Set up authenticator</a>
    </section>{:else}<div class="review-notice">
      <Shield size={20} />
      <p>
        {data.integrations?.identity === 'sandbox'
          ? 'Sandbox creator approval does not establish real identity verification.'
          : 'Profile review and hosted identity verification are separate decisions.'}
        {data.integrations?.payments === 'sandbox'
          ? 'Sandbox payout approval transfers no real funds.'
          : 'Stripe manages provider payouts. Financial reconciliation requires server-confirmed provider records.'}
        Every decision is audited.
      </p>
    </div>
    <section class="panel">
      <div class="review-tabs" aria-label="Review queues">
        {#each [{ id: 'creator' as const, label: 'Creators' }, { id: 'media' as const, label: 'Media' }, { id: 'report' as const, label: 'Reports' }, { id: 'payout' as const, label: 'Payouts' }, { id: 'checkout' as const, label: 'Payments' }, { id: 'finance' as const, label: 'Reconciliation' }, { id: 'appeals' as const, label: 'Appeals' }, { id: 'operations' as const, label: 'Operations' }, { id: 'privacy' as const, label: 'Privacy' }] as item (item.id)}<button
            class={['button-secondary', { active: tab === item.id }]}
            onclick={() => {
              selectQueue(item.id);
            }}
            aria-pressed={tab === item.id}>{item.label}</button
          >{/each}{#if tab !== 'operations' && tab !== 'appeals' && tab !== 'finance' && tab !== 'privacy'}<button
            class="icon-button"
            onclick={() => refreshAll()}
            aria-label="Refresh review queues"><RefreshCw size={17} /></button
          >{/if}
      </div>
      {#if tab === 'privacy'}{#key data.user.id}<AdminPrivacy
          />{/key}{:else if tab === 'finance'}{#key data.user.id}<AdminFinance
          />{/key}{:else if tab === 'operations'}{#key data.user.id}<AdminOperations
          />{/key}{:else if tab === 'appeals'}{#key data.user.id}<AdminAppeals
          />{/key}{:else}{#if !items.length}<div class="empty-panel">
            <Check size={28} />
            <h2>This queue is clear.</h2>
            <p>New review requests will appear here.</p>
          </div>{/if}{#each items as item (item.id)}<article class="review-item">
            <div class="list-row">
              <div>
                <strong
                  >{item.display_name ??
                    (tab === 'payout'
                      ? 'Local payout request'
                      : tab === 'report'
                        ? 'Content report'
                        : tab === 'checkout'
                          ? sandboxPayment(item)
                            ? 'Sandbox payment'
                            : 'Stripe payment'
                          : 'Private media')}</strong
                >{#if item.buyer}<p>{item.buyer} → {item.creator} · {item.kind}</p>{/if}<small
                  >{item.handle ? `@${item.handle} · ` : ''}{item.id}</small
                >{#if item.created_at}<small>{new Date(item.created_at).toLocaleString()}</small
                  >{/if}{#if item.amount_minor !== undefined}<p>
                    ${(item.amount_minor / 100).toFixed(2)}
                  </p>{/if}{#if item.reason}<p>{item.reason}</p>{/if}{#if item.post_id}<button
                    class="button-secondary"
                    onclick={() => (draft.inspectedPost = item.post_id ?? null)}
                    >Inspect reported post</button
                  ><a class="text-button" href={`/?post=${item.post_id}#post-${item.post_id}`}
                    >Open public post</a
                  >{/if}
              </div>
              <span class="status-pill"
                >{item.verification_status ?? item.state ?? item.status ?? 'pending'}</span
              >{#if tab !== 'checkout' || item.status === 'paid'}<button
                  class="button-secondary"
                  onclick={() => {
                    draft.selected = draft.selected === item.id ? null : item.id;
                    draft.reason = '';
                  }}>{tab === 'checkout' ? 'Refund' : 'Review'}</button
                >{/if}
            </div>
            {#if tab === 'creator' && item.review_context}<div class="review-context">
                <strong>Previous review or resubmission: {item.review_context.decision}</strong>
                <p>{item.review_context.reason}</p>
                <small>{new Date(item.review_context.created_at).toLocaleString()}</small>
              </div>{/if}
            {#if draft.selected === item.id}<div class="review-form">
                {#if tab === 'checkout'}<p class="footnote">
                    {sandboxPayment(item)
                      ? 'Confirm a full sandbox refund'
                      : 'Request a full provider refund'} of ${(
                      (item.amount_minor ?? 0) / 100
                    ).toFixed(2)}.
                    {sandboxPayment(item)
                      ? 'Paid access from this payment will be revoked. No real money is moved.'
                      : 'This requests a refund of the paid period or post and its application fee. Final funds and access status depend on server-confirmed provider results.'}
                  </p>
                  <label
                    >Refund reason<textarea
                      aria-label="Refund reason"
                      bind:value={draft.reason}
                      minlength="5"
                      maxlength="500"
                      required></textarea></label
                  >
                  <div class="form-actions">
                    <button
                      class="button-primary"
                      onclick={() => refund(item.id)}
                      disabled={busy || draft.reason.trim().length < 5}
                      >{sandboxPayment(item)
                        ? 'Confirm sandbox refund'
                        : 'Confirm provider refund request'}</button
                    ><button class="button-secondary" onclick={() => (draft.selected = null)}
                      >Keep payment</button
                    >
                  </div>{:else}
                  {#if item.content_type?.startsWith('video/')}
                    <video
                      class="review-preview"
                      controls
                      playsinline
                      preload="metadata"
                      poster={item.preview_url}
                      src={item.full_url ?? `/api/platform/media/${item.id}/full`}
                      ><track
                        kind="captions"
                        src={item.captions_ready
                          ? `/api/platform/media/${item.id}/captions`
                          : undefined}
                        srclang="en"
                        label="English"
                        default
                      />Your browser does not support this video.</video
                    >
                  {:else if item.preview_url}<img
                      class="review-preview"
                      src={item.preview_url}
                      alt="Media under review"
                    />{/if}<label
                    >{tab === 'creator'
                      ? 'Reason shared with the creator'
                      : 'Decision reason'}<textarea
                      bind:value={draft.reason}
                      minlength="10"
                      maxlength="2000"
                      placeholder={tab === 'creator'
                        ? 'Explain the decision to the creator…'
                        : 'Document the decision for the audit trail…'}></textarea></label
                  >
                  {#if tab === 'creator'}<p class="footnote">
                      This explanation is visible to the creator for approval and rejection. Keep
                      private investigation notes and other people’s personal information out of it.
                    </p>{/if}
                  <div class="form-actions">
                    <button
                      class="button-primary"
                      disabled={busy || draft.reason.trim().length < 10}
                      onclick={() => review(item.id, 'approve')}
                      ><Check size={16} />{tab === 'report'
                        ? 'Uphold report / remove content'
                        : data.integrations?.identity === 'sandbox' &&
                            data.integrations?.payments === 'sandbox'
                          ? 'Approve in sandbox'
                          : 'Approve review'}</button
                    ><button
                      class="button-secondary"
                      disabled={busy || draft.reason.trim().length < 10}
                      onclick={() => review(item.id, 'reject')}
                      ><X size={16} />{tab === 'report' ? 'Dismiss report' : 'Reject'}</button
                    >
                  </div>
                {/if}
              </div>{/if}
          </article>{/each}<CollectionPages
          nextCursor={data.reviews?.next_cursors?.[collection]}
          parameter={`${collection}_cursor`}
          label={`${collection} review`}
          nextLabel="Next review page"
          firstLabel="Back to first review page"
        />{/if}
    </section>{/if}
</main>
{#if draft.inspectedPost && data.user?.role === 'admin' && data.user.mfa_enabled}{#key `${data.user.id}:${draft.inspectedPost}`}<AdminPostInspection
      postId={draft.inspectedPost}
      viewerId={data.user.id}
      onclose={() => (draft.inspectedPost = null)}
    />{/key}{/if}

<style>
  .review-context {
    padding: 14px 0;
    font-size: 12px;
    overflow-wrap: anywhere;
  }
  .review-context p {
    margin: 8px 0;
  }
  .review-notice {
    display: flex;
    align-items: start;
    gap: 12px;
    padding: 18px;
    border-radius: 12px;
    background: #edf2e5;
    margin-bottom: 22px;
    font-size: 12px;
    line-height: 1.8;
    color: #5d7150;
  }
  .review-tabs {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
    margin-bottom: 12px;
  }
  .review-tabs .active {
    background: #e4ebdc;
    border-color: #aebda0;
  }
  .review-tabs .icon-button {
    margin-left: auto;
  }
  .review-item .list-row p {
    margin-top: 10px;
  }
  .review-item .list-row {
    flex-wrap: wrap;
  }
  .review-item .list-row > div {
    flex: 1;
    min-width: 180px;
  }
  .review-form {
    display: grid;
    gap: 16px;
    padding: 12px 0 24px;
  }
  .review-form label {
    display: grid;
    gap: 8px;
    font-size: 12px;
    font-weight: 600;
  }
  .review-form textarea {
    width: 100%;
    border: 1px solid var(--border);
    background: var(--background);
    padding: 12px;
    border-radius: 8px;
    resize: vertical;
    min-height: 90px;
    font-weight: 400;
  }
  .review-preview {
    max-height: 350px;
    border-radius: 12px;
    object-fit: contain;
  }
</style>
