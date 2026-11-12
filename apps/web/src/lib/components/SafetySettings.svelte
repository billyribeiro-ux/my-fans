<script lang="ts">
  import { onMount } from 'svelte';
  import { refreshAll } from '$app/navigation';
  import Dialog from './Dialog.svelte';
  import LoadOlder from './LoadOlder.svelte';
  interface Safety {
    blocks: { id: string; display_name: string; created_at: string }[];
    appeals: {
      id: string;
      post_id: string;
      caption: string;
      reason: string;
      status: string;
      decision_reason: string | null;
      created_at: string;
    }[];
    blocked_posts: { id: string; caption: string; appeal_pending: boolean }[];
    next_cursors?: Record<'blocks' | 'appeals' | 'blocked_posts', string | null>;
  }
  let safety = $state.raw<Safety | null>(null);
  let loading = $state(false);
  let busy = $state(false);
  let error = $state('');
  let message = $state('');
  let selected = $state<{ kind: 'appeal' | 'unblock'; id: string; label: string } | null>(null);
  let reason = $state('');
  let controller: AbortController | undefined;
  let disposed = false;
  const mutations = new AbortController();
  async function load(collection?: 'blocks' | 'appeals' | 'blocked_posts', cursor?: string) {
    if (disposed || (cursor && (busy || loading))) return;
    controller?.abort();
    const current = new AbortController();
    controller = current;
    loading = true;
    error = '';
    try {
      const query =
        cursor && collection ? `?${collection}_cursor=${encodeURIComponent(cursor)}` : '';
      const response = await fetch(`/api/platform/account/safety${query}`, {
        signal: current.signal,
      });
      const result = await response.json();
      if (disposed || current.signal.aborted) return;
      if (response.status === 401 || response.status === 403) {
        safety = null;
        selected = null;
        reason = '';
      }
      if (!response.ok) throw new Error(result.message ?? 'Safety settings could not be loaded.');
      if (!current.signal.aborted) {
        if (cursor && collection && safety) {
          const existing = safety[collection];
          safety = {
            ...safety,
            [collection]: [
              ...existing,
              ...result[collection].filter(
                (item: { id: string }) => !existing.some((old) => old.id === item.id),
              ),
            ],
            next_cursors: {
              ...safety.next_cursors,
              [collection]: result.next_cursors?.[collection] ?? null,
            },
          } as Safety;
        } else safety = result;
      }
    } catch (e) {
      if (!disposed && !current.signal.aborted)
        error = e instanceof Error ? e.message : 'Safety settings could not be loaded.';
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
      safety = null;
      selected = null;
      reason = '';
    };
  });
  function confirm(value: NonNullable<typeof selected>) {
    selected = value;
    reason = '';
    error = '';
    message = '';
  }
  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (disposed || !selected || busy) return;
    const decision = selected;
    busy = true;
    error = '';
    message = '';
    try {
      const response = await fetch(
        decision.kind === 'appeal'
          ? '/api/platform/appeals'
          : `/api/platform/users/${decision.id}/block`,
        {
          method: decision.kind === 'appeal' ? 'POST' : 'DELETE',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify(
            decision.kind === 'appeal' ? { post_id: decision.id, reason: reason.trim() } : {},
          ),
          signal: mutations.signal,
        },
      );
      const result = await response.json();
      if (disposed) return;
      if (!response.ok) throw new Error(result.message ?? 'The safety action could not be saved.');
      selected = null;
      message =
        decision.kind === 'appeal'
          ? 'Appeal submitted for review. The post remains hidden while the appeal is pending.'
          : 'Your block has been removed. Any block by the other person still applies.';
      await Promise.all([load(), refreshAll()]);
    } catch (e) {
      if (!disposed)
        error = e instanceof Error ? e.message : 'The safety action could not be saved.';
    } finally {
      if (!disposed) busy = false;
    }
  }
</script>

<section class="panel" aria-labelledby="safety-heading" aria-busy={loading}>
  <div class="heading-row">
    <h2 id="safety-heading">Community safety</h2>
    <button class="text-button" onclick={() => load()} disabled={loading || busy}
      >Refresh safety settings</button
    >
  </div>
  <p class="section-description">
    Blocking hides a person’s posts and discovery profile, prevents new messages and post
    interactions, and keeps conversation history readable. Existing paid media access remains
    unchanged; blocking makes no financial changes.
  </p>
  {#if error && !selected}<p class="error-message" role="alert">{error}</p>{/if}
  {#if message}<p class="status-message" role="status">{message}</p>{/if}
  {#if loading && !safety}<p role="status">Loading safety settings…</p>{/if}
  {#if safety}
    <h3>People you blocked</h3>
    {#if !safety.blocks.length}<p class="footnote">
        You have not blocked anyone. Manage blocks from a conversation in <a
          href="/messages"
          class="text-button">Messages</a
        >.
      </p>{/if}
    {#each safety.blocks as person (person.id)}<div class="safety-row">
        <div>
          <strong>{person.display_name}</strong><small
            >Blocked {new Date(person.created_at).toLocaleDateString()}</small
          >
        </div>
        <button
          class="button-secondary"
          disabled={busy}
          onclick={() => confirm({ kind: 'unblock', id: person.id, label: person.display_name })}
          >Unblock user</button
        >
      </div>{/each}
    <LoadOlder
      cursor={safety.next_cursors?.blocks}
      loading={loading || busy}
      label="More blocked people"
      onload={(cursor) => load('blocks', cursor)}
    />
    <h3>Content appeals</h3>
    <p class="footnote">
      Ask for review of a moderation block on your own post. An accepted appeal returns the post to
      drafts; it does not publish it.
    </p>
    {#each safety.blocked_posts as post (post.id)}<div class="safety-row">
        <p>{post.caption || 'Post without caption'}</p>
        {#if post.appeal_pending}<span class="status-pill">Appeal pending</span>{:else}<button
            class="button-secondary"
            disabled={busy}
            onclick={() => confirm({ kind: 'appeal', id: post.id, label: post.caption })}
            >Appeal removal</button
          >{/if}
      </div>{/each}
    {#if !safety.blocked_posts.length}<p class="footnote">
        No blocked posts available for appeal.
      </p>{/if}
    <LoadOlder
      cursor={safety.next_cursors?.blocked_posts}
      loading={loading || busy}
      label="Older blocked posts"
      onload={(cursor) => load('blocked_posts', cursor)}
    />
    {#each safety.appeals as appeal (appeal.id)}<article class="appeal-record">
        <div class="heading-row">
          <strong>{appeal.caption || 'Content appeal'}</strong><span class="status-pill"
            >{appeal.status}</span
          >
        </div>
        <small>Submitted {new Date(appeal.created_at).toLocaleString()}</small>
        <p>{appeal.reason}</p>
        {#if appeal.decision_reason}<p>
            <strong>Review decision:</strong>
            {appeal.decision_reason}
          </p>{/if}
      </article>{/each}
    <LoadOlder
      cursor={safety.next_cursors?.appeals}
      loading={loading || busy}
      label="Older appeals"
      onload={(cursor) => load('appeals', cursor)}
    />
  {/if}
</section>
{#if selected}<Dialog
    title={selected.kind === 'appeal' ? 'Request a content review' : 'Unblock this person?'}
    onclose={() => {
      if (!busy) selected = null;
    }}
  >
    <form class="form-grid" onsubmit={submit}>
      <p class="selected-label">{selected.label}</p>
      {#if error}<p class="error-message" role="alert">{error}</p>{/if}
      {#if selected.kind === 'appeal'}<p>
          Explain why this moderation decision should be reviewed. Your post stays hidden until
          review. Acceptance returns it to drafts.
        </p>
        <label
          >Appeal reason<textarea
            bind:value={reason}
            required
            minlength="5"
            maxlength="2000"
            placeholder="Explain the context and why the post should be reviewed…"
          ></textarea></label
        >{:else}<p>
          Remove your block to allow this person’s posts and profile to appear again. New messages
          remain restricted if the other person has blocked you or their account is disabled. Paid
          access is unchanged.
        </p>{/if}
      <div class="form-actions">
        <button
          class="button-primary"
          disabled={busy || (selected.kind === 'appeal' && reason.trim().length < 5)}
          >{selected.kind === 'appeal' ? 'Submit appeal' : 'Confirm unblock'}</button
        ><button
          class="button-secondary"
          type="button"
          disabled={busy}
          onclick={() => (selected = null)}>Cancel</button
        >
      </div>
    </form>
  </Dialog>{/if}

<style>
  .heading-row,
  .safety-row {
    display: flex;
    justify-content: space-between;
    align-items: start;
    gap: 12px;
    flex-wrap: wrap;
  }
  .heading-row h2 {
    margin-bottom: 0;
  }
  .section-description {
    margin: 14px 0 20px;
  }
  h3 {
    font-size: 15px;
    margin: 23px 0 12px;
  }
  .safety-row,
  .appeal-record {
    border-top: 1px solid var(--border);
    padding: 16px 0;
  }
  .safety-row > div,
  .safety-row > p {
    flex: 1;
    min-width: 140px;
  }
  strong,
  .selected-label {
    overflow-wrap: anywhere;
    font-size: 13px;
  }
  small {
    display: block;
    margin-top: 7px;
    font-size: 11px;
    color: var(--muted);
  }
  .appeal-record p {
    margin-top: 12px;
    overflow-wrap: anywhere;
  }
</style>
