<script lang="ts">
  import { onMount } from 'svelte';
  import Dialog from './Dialog.svelte';
  import { parseAdminPost, type AdminPostInspection } from '#lib/admin-post.ts';
  let { postId, viewerId, onclose }: { postId: string; viewerId: string; onclose: () => void } =
    $props();
  let inspection = $state.raw<AdminPostInspection | null>(null);
  let loading = $state(false);
  let error = $state('');
  let needsAssurance = $state(false);
  let mediaLoading = $state(false);
  let mediaFailed = $state(false);
  let controller: AbortController | undefined;
  let disposed = false;
  async function load() {
    controller?.abort();
    const current = new AbortController();
    controller = current;
    const target = postId,
      actor = viewerId;
    inspection = null;
    loading = true;
    mediaLoading = false;
    mediaFailed = false;
    error = '';
    needsAssurance = false;
    try {
      const response = await fetch(`/api/platform/admin/posts/${target}`, {
        signal: current.signal,
        cache: 'no-store',
      });
      const result = await response.json();
      if (disposed || current.signal.aborted || actor !== viewerId || target !== postId) return;
      needsAssurance = response.status === 401 || response.status === 403;
      if (!response.ok)
        throw new Error(
          response.status === 404
            ? 'This post is unavailable for inspection.'
            : needsAssurance
              ? 'Recent administrator authenticator verification is required to inspect this post.'
              : 'Post inspection could not be loaded. Retry before making a decision.',
        );
      inspection = parseAdminPost(result, actor, target);
      mediaLoading = !!inspection.post.media_url;
    } catch (e) {
      if (!disposed && !current.signal.aborted)
        error = e instanceof Error ? e.message : 'Post inspection could not be loaded.';
    } finally {
      if (!disposed && !current.signal.aborted) loading = false;
    }
  }
  function mediaReady(failed = false) {
    mediaLoading = false;
    mediaFailed = failed;
  }
  onMount(() => {
    void load();
    return () => {
      disposed = true;
      controller?.abort();
      inspection = null;
    };
  });
</script>

<Dialog title="Inspect reported post" {onclose}>
  <div class="inspection" aria-busy={loading}>
    <p class="footnote">
      Private moderator inspection is audited. It does not purchase content, grant subscriber
      access, or change the post.
    </p>
    {#if loading}<p role="status">Loading reported post…</p>{/if}
    {#if error}<p class="error-message" role="alert">{error}</p>
      {#if needsAssurance}<a class="text-button" href="/settings#mfa-heading"
          >Refresh authenticator verification</a
        >{/if}
      <button class="button-secondary" onclick={load} disabled={loading}>Retry inspection</button>
    {/if}
    {#if inspection}
      <h3>{inspection.creator.display_name}</h3>
      <p class="caption">{inspection.post.caption}</p>
      <dl>
        <div>
          <dt>Publication</dt>
          <dd>{inspection.post.state}</dd>
        </div>
        <div>
          <dt>Audience</dt>
          <dd>{inspection.post.visibility}</dd>
        </div>
        <div>
          <dt>Moderation block</dt>
          <dd>{inspection.post.moderation_blocked ? 'Blocked' : 'None'}</dd>
        </div>
        {#if inspection.post.media_state}<div>
            <dt>Media status</dt>
            <dd>{inspection.post.media_state}</dd>
          </div>{/if}
      </dl>
      {#if inspection.post.media_url}
        {#if inspection.post.media_type === 'video'}
          <video
            controls
            playsinline
            preload="metadata"
            src={inspection.post.media_url}
            poster={inspection.post.preview_url || undefined}
            onloadedmetadata={() => mediaReady()}
            onerror={() => mediaReady(true)}
          >
            <track
              kind="captions"
              src={inspection.post.captions_url ?? undefined}
              srclang="en"
              label="English"
              default
            />Your browser does not support this video.
          </video>
        {:else}<img
            src={inspection.post.media_url}
            alt="Full media attached to the reported post"
            onload={() => mediaReady()}
            onerror={() => mediaReady(true)}
          />{/if}
        {#if mediaLoading}<p role="status">Loading media evidence…</p>{/if}
        {#if mediaFailed}<p class="error-message" role="alert">
            Media evidence could not be loaded. Retry inspection before deciding.
          </p>
          <button class="button-secondary" onclick={load}>Retry media evidence</button>{/if}
      {:else}<p class="footnote">
          No renderable attached media is available. Review the caption and recorded media status;
          unavailable evidence does not confirm that content is safe.
        </p>{/if}
      <small>Post reference: {inspection.post.id}</small>
    {/if}
    <div class="form-actions">
      <button class="button-secondary" onclick={onclose}>Close inspection</button>
    </div>
  </div>
</Dialog>

<style>
  .inspection {
    display: grid;
    gap: 16px;
    overflow-wrap: anywhere;
  }
  .caption {
    white-space: pre-wrap;
  }
  img,
  video {
    width: 100%;
    max-height: 420px;
    object-fit: contain;
    border-radius: 12px;
    background: var(--background);
  }
  dl {
    display: grid;
    gap: 8px;
    margin: 0;
    font-size: 13px;
  }
  dl div {
    display: flex;
    justify-content: space-between;
    gap: 16px;
  }
  dt {
    color: var(--muted);
  }
  dd {
    margin: 0;
    text-align: right;
  }
</style>
