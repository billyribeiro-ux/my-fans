<script lang="ts">
  import { onMount } from 'svelte';
  import { refreshAll } from '$app/navigation';
  import Dialog from './Dialog.svelte';
  import LoadOlder from './LoadOlder.svelte';
  interface Appeal {
    id: string;
    post_id: string;
    display_name: string;
    caption: string;
    reason: string;
    status: string;
    created_at: string;
    full_url?: string | null;
    preview_url?: string | null;
    media_type?: 'image' | 'video';
    captions_url?: string | null;
    moderation_reason?: string | null;
  }
  let appeals = $state.raw<Appeal[] | null>(null);
  let loading = $state(false);
  let busy = $state(false);
  let error = $state('');
  let message = $state('');
  let needsSignIn = $state(false);
  let selected = $state<{ appeal: Appeal; decision: 'accept' | 'reject' } | null>(null);
  let reason = $state('');
  let controller: AbortController | undefined;
  let disposed = false;
  const mutations = new AbortController();
  let pendingMedia = $state.raw<string[]>([]);
  let failedMedia = $state.raw<string[]>([]);
  let evidenceVersion = $state(0);
  let nextCursor = $state<string | null>(null);
  function mediaReady(id: string, failed = false) {
    pendingMedia = pendingMedia.filter((value) => value !== id);
    failedMedia = failed
      ? [...failedMedia.filter((value) => value !== id), id]
      : failedMedia.filter((value) => value !== id);
  }
  async function load(cursor?: string) {
    if (disposed || (cursor && (busy || loading))) return;
    controller?.abort();
    const current = new AbortController();
    controller = current;
    loading = true;
    error = '';
    try {
      const response = await fetch(
        `/api/platform/admin/appeals${cursor ? `?cursor=${encodeURIComponent(cursor)}` : ''}`,
        { signal: current.signal },
      );
      const result = await response.json();
      if (disposed || current.signal.aborted) return;
      needsSignIn = response.status === 401 || response.status === 403;
      if (needsSignIn) {
        appeals = null;
        nextCursor = null;
        selected = null;
        pendingMedia = [];
        failedMedia = [];
      }
      if (!response.ok) throw new Error(result.message ?? 'Appeals could not be loaded.');
      const incoming = result.appeals.filter(
        (appeal: Appeal) => !cursor || !appeals?.some((old) => old.id === appeal.id),
      );
      pendingMedia = [
        ...(cursor ? pendingMedia : []),
        ...incoming.filter((appeal: Appeal) => appeal.full_url).map((appeal: Appeal) => appeal.id),
      ];
      if (!cursor) {
        failedMedia = [];
        evidenceVersion++;
      }
      appeals = cursor ? [...(appeals ?? []), ...incoming] : incoming;
      nextCursor = result.next_cursor ?? null;
    } catch (e) {
      if (!disposed && !current.signal.aborted)
        error = e instanceof Error ? e.message : 'Appeals could not be loaded.';
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
      appeals = null;
      selected = null;
      reason = '';
    };
  });
  function confirm(appeal: Appeal, decision: 'accept' | 'reject') {
    selected = { appeal, decision };
    reason = '';
    error = '';
    message = '';
  }
  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (disposed || !selected || busy || reason.trim().length < 5) return;
    const review = selected;
    busy = true;
    error = '';
    message = '';
    try {
      const response = await fetch(`/api/platform/admin/appeals/${review.appeal.id}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ decision: review.decision, reason: reason.trim() }),
        signal: mutations.signal,
      });
      const result = await response.json();
      if (disposed) return;
      needsSignIn = response.status === 401 || response.status === 403;
      if (!response.ok) throw new Error(result.message ?? 'Appeal decision could not be saved.');
      selected = null;
      message =
        review.decision === 'accept'
          ? 'Appeal accepted. The moderation block is cleared and the post remains a draft.'
          : 'Appeal rejected. The post remains blocked.';
      await Promise.all([load(), refreshAll()]);
    } catch (e) {
      if (!disposed) error = e instanceof Error ? e.message : 'Appeal decision could not be saved.';
    } finally {
      if (!disposed) busy = false;
    }
  }
</script>

<section aria-labelledby="appeals-heading" aria-busy={loading}>
  <div class="heading-row">
    <h2 id="appeals-heading">Content appeals</h2>
    <button class="button-secondary" onclick={() => load()} disabled={loading || busy}
      >Refresh appeals</button
    >
  </div>
  <p class="footnote">
    Review the creator’s context and content before deciding. Every decision is audited. Acceptance
    clears the moderation block and returns content to drafts. Administrator verification expires
    after 15 minutes.
  </p>
  {#if error && !selected}<p class="error-message" role="alert">
      {error}{#if needsSignIn}
        <a href="/auth">Sign in again with your authenticator</a>.{/if}
    </p>{/if}
  {#if message}<p class="status-message" role="status">{message}</p>{/if}
  {#if loading && !appeals}<p role="status">Loading appeals…</p>{/if}
  {#if appeals?.length === 0}<div class="empty-panel">
      <h3>No appeals to review.</h3>
      <p>Creator appeals will appear here.</p>
    </div>{/if}
  {#each appeals ?? [] as appeal (appeal.id)}<article class="appeal-row">
      <div class="heading-row">
        <strong>{appeal.display_name}</strong><span class="status-pill">{appeal.status}</span>
      </div>
      <p class="caption">{appeal.caption}</p>
      <p>
        <strong>Prior moderation context:</strong>
        {appeal.moderation_reason ??
          'The prior moderation reason is unavailable. Review the available post and appeal evidence before deciding.'}
      </p>
      {#if appeal.full_url}
        {#key `${appeal.id}:${evidenceVersion}`}
          {#if appeal.media_type === 'video'}<video
              class="appeal-media"
              controls
              playsinline
              preload="metadata"
              src={appeal.full_url}
              poster={appeal.preview_url ?? undefined}
              onloadedmetadata={() => mediaReady(appeal.id)}
              onerror={() => mediaReady(appeal.id, true)}
              ><track
                kind="captions"
                src={appeal.captions_url ?? undefined}
                srclang="en"
                label="English"
                default
              />Your browser does not support this video.</video
            >{:else}<img
              class="appeal-media"
              src={appeal.full_url}
              alt="Media attached to the post under appeal"
              onload={() => mediaReady(appeal.id)}
              onerror={() => mediaReady(appeal.id, true)}
            />{/if}
        {/key}
        {#if pendingMedia.includes(appeal.id)}<p class="footnote" role="status">
            Loading media evidence…
          </p>{/if}
        {#if failedMedia.includes(appeal.id)}<p class="error-message" role="alert">
            Media evidence could not be loaded. Refresh appeals to retry before making a decision.
          </p>{/if}
      {:else}<p class="footnote">
          No attached media evidence is available for this appeal. Review the post caption and
          creator’s explanation.
        </p>{/if}
      <p><strong>Creator’s reason:</strong> {appeal.reason}</p>
      <small>Post {appeal.post_id} · {new Date(appeal.created_at).toLocaleString()}</small
      >{#if appeal.status === 'pending'}<div class="form-actions">
          <button
            class="button-primary"
            disabled={busy || pendingMedia.includes(appeal.id) || failedMedia.includes(appeal.id)}
            onclick={() => confirm(appeal, 'accept')}>Review acceptance</button
          ><button
            class="button-secondary"
            disabled={busy || pendingMedia.includes(appeal.id) || failedMedia.includes(appeal.id)}
            onclick={() => confirm(appeal, 'reject')}>Review rejection</button
          >
        </div>{/if}
    </article>{/each}
  <LoadOlder cursor={nextCursor} loading={loading || busy} label="More appeals" onload={load} />
</section>
{#if selected}<Dialog
    title={selected.decision === 'accept' ? 'Accept this appeal?' : 'Reject this appeal?'}
    onclose={() => {
      if (!busy) selected = null;
    }}
    ><form class="form-grid" onsubmit={submit}>
      <p>{selected.appeal.caption}</p>
      <p>
        {selected.decision === 'accept'
          ? 'Clear this post’s moderation block and return it to drafts. This will not publish the post.'
          : 'Keep this post blocked and record why the appeal was rejected.'}
      </p>
      {#if error}<p class="error-message" role="alert">
          {error}{#if needsSignIn}
            <a href="/auth">Sign in again with your authenticator</a>.{/if}
        </p>{/if}<label
        >Appeal decision reason<textarea
          bind:value={reason}
          required
          minlength="5"
          maxlength="2000"
          placeholder="Record the reviewed evidence and decision…"></textarea></label
      >
      <div class="form-actions">
        <button class="button-primary" disabled={busy || reason.trim().length < 5}
          >Confirm {selected.decision === 'accept' ? 'acceptance' : 'rejection'}</button
        ><button
          class="button-secondary"
          type="button"
          disabled={busy}
          onclick={() => (selected = null)}>Cancel</button
        >
      </div>
    </form></Dialog
  >{/if}

<style>
  section {
    padding-top: 8px;
  }
  .heading-row {
    display: flex;
    justify-content: space-between;
    align-items: start;
    gap: 12px;
    flex-wrap: wrap;
  }
  .footnote {
    margin: 12px 0 22px;
  }
  .appeal-row {
    padding: 22px 0;
    border-top: 1px solid var(--border);
    overflow-wrap: anywhere;
  }
  .appeal-row p {
    margin: 14px 0;
  }
  .caption {
    font-weight: 600;
  }
  .appeal-media {
    display: block;
    width: 100%;
    max-height: 420px;
    object-fit: contain;
    border-radius: 12px;
    background: var(--background);
  }
  small {
    display: block;
    color: var(--muted);
    font-size: 11px;
    margin-bottom: 16px;
  }
</style>
