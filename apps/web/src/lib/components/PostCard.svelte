<script lang="ts">
  import {
    Heart,
    MessageCircle,
    Bookmark,
    ArrowUpRight,
    LockKeyhole,
    Send,
    Check,
    Flag,
  } from '@lucide/svelte';
  import { goto, refreshAll } from '$app/navigation';
  import { page } from '$app/state';
  import type { FeedCreator, FeedPost } from '#lib/contracts.ts';
  import { onMount } from 'svelte';
  import LoadOlder from './LoadOlder.svelte';
  import PrivateVideo from './PrivateVideo.svelte';
  let {
    post,
    creator,
    liked,
    saved,
    onlike,
    onsave,
    onprofile,
    onsubscribe,
    authenticated,
  }: {
    post: FeedPost;
    creator: FeedCreator;
    liked: boolean;
    saved: boolean;
    onlike: () => void;
    onsave: () => void;
    onprofile: () => void;
    onsubscribe: () => void;
    authenticated: boolean;
  } = $props();
  let commentsOpen = $state(false);
  let comment = $state('');
  let comments = $state.raw<{ id: string; display_name: string; body: string }[]>([]);
  let nextCommentCursor = $state<string | null>(null);
  let loadingComments = $state(false);
  let commentsController: AbortController | undefined;
  let disposed = false;
  const mutations = new AbortController();
  onMount(() => () => {
    disposed = true;
    commentsController?.abort();
    mutations.abort();
    comment = '';
    reportReason = '';
    comments = [];
  });
  let busy = $state(false);
  let error = $state('');
  let copied = $state(false);
  let shareError = $state('');
  let reportOpen = $state(false);
  let reportReason = $state('');
  let reportSent = $state(false);
  let imageUrl = $derived(post.locked ? post.preview_url : (post.media_url ?? post.preview_url));
  let date = $derived(
    new Intl.DateTimeFormat('en-US', { month: 'short', day: 'numeric', timeZone: 'UTC' }).format(
      new Date(post.created_at),
    ),
  );
  async function fetchComments(cursor?: string) {
    if (disposed || (cursor && loadingComments)) return;
    commentsController?.abort();
    const current = new AbortController();
    commentsController = current;
    const id = post.id;
    loadingComments = true;
    error = '';
    try {
      const response = await fetch(
        `/api/platform/posts/${id}/comments${cursor ? `?cursor=${encodeURIComponent(cursor)}` : ''}`,
        { signal: current.signal },
      );
      const result = await response.json();
      if (!response.ok) throw new Error(result.message ?? 'Unable to load comments.');
      if (disposed || current.signal.aborted || id !== post.id) return;
      comments = cursor
        ? [
            ...result.comments.filter(
              (item: { id: string }) => !comments.some((old) => old.id === item.id),
            ),
            ...comments,
          ]
        : result.comments;
      nextCommentCursor = result.next_cursor ?? null;
    } catch (e) {
      if (!current.signal.aborted && id === post.id)
        error = e instanceof Error ? e.message : 'Unable to load comments.';
    } finally {
      if (!current.signal.aborted) loadingComments = false;
    }
  }
  function loadComments() {
    commentsOpen = !commentsOpen;
    if (commentsOpen) void fetchComments();
    else {
      commentsController?.abort();
      loadingComments = false;
    }
  }
  async function addComment(event: SubmitEvent) {
    event.preventDefault();
    if (!authenticated) {
      void goto('/auth');
      return;
    }
    if (!comment.trim() || busy || disposed) return;
    const target = post.id;
    busy = true;
    error = '';
    try {
      const response = await fetch(`/api/platform/posts/${target}/comments`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ body: comment.trim() }),
        signal: mutations.signal,
      });
      const result = await response.json();
      if (disposed || target !== post.id) return;
      if (!response.ok) throw new Error(result.message ?? 'Unable to add comment.');
      comment = '';
      await fetchComments();
      if (disposed) return;
      await refreshAll();
    } catch (e) {
      if (!disposed) error = e instanceof Error ? e.message : 'Unable to add comment.';
    } finally {
      if (!disposed) busy = false;
    }
  }
  async function report(event: SubmitEvent) {
    event.preventDefault();
    if (disposed || busy) return;
    if (!authenticated) {
      void goto('/auth');
      return;
    }
    busy = true;
    const target = post.id;
    error = '';
    try {
      const response = await fetch('/api/platform/reports', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ post_id: target, reason: reportReason }),
        signal: mutations.signal,
      });
      const result = await response.json();
      if (disposed || target !== post.id) return;
      if (!response.ok) throw new Error(result.message ?? 'Unable to submit report.');
      reportSent = true;
      reportOpen = false;
    } catch (e) {
      if (!disposed) error = e instanceof Error ? e.message : 'Unable to submit report.';
    } finally {
      if (!disposed) busy = false;
    }
  }
  async function share() {
    try {
      await navigator.clipboard.writeText(`${location.origin}/?post=${post.id}#post-${post.id}`);
      if (disposed) return;
      copied = true;
      shareError = '';
    } catch {
      if (!disposed) shareError = 'Copy is unavailable in this browser.';
    }
  }
</script>

<article class="post" id={`post-${post.id}`}>
  <div class="post-header">
    <button class="creator" onclick={onprofile}
      >{#if creator.avatar_url}<img
          class="avatar"
          src={creator.avatar_url}
          alt=""
          width="44"
          height="44"
        />{:else}<span class="avatar avatar-fallback" aria-hidden="true"
          >{creator.display_name.charAt(0)}</span
        >{/if}<span
        ><strong>{creator.display_name}</strong><small>@{creator.handle}<span>·</span>{date}</small
        ></span
      ></button
    ><span class="category">{creator.category}</span>
  </div>
  <p class="caption">{post.caption}</p>
  <div class={['post-media', { locked: post.locked }]}>
    {#if post.media_type === 'video' && !post.locked && post.media_url}
      <PrivateVideo
        poster={post.preview_url}
        label={`${creator.display_name}'s video`}
        src={post.media_url}
        hlsUrl={post.hls_url}
        captionsUrl={post.captions_url}
      />
    {:else if imageUrl}<img
        src={imageUrl}
        alt={`${creator.display_name}'s ${post.locked ? 'public preview' : 'post'}`}
        loading="lazy"
        width="900"
        height="600"
      />{:else}<div class="media-fallback">A little inspiration, just for you.</div>{/if}
    {#if post.locked}<div class="locked-content">
        <span class="lock-circle"><LockKeyhole size={24} /></span><strong
          >A little closer to {creator.display_name.split(' ')[0]}</strong
        >
        <p>
          {post.visibility === 'purchase'
            ? 'This post is available as a paid unlock.'
            : 'This post is for subscribers.'}
        </p>
        <button class="button-light" onclick={onsubscribe}
          ><LockKeyhole size={14} />{post.visibility === 'purchase'
            ? `Unlock · $${((post.price_minor ?? 0) / 100).toFixed(2)}`
            : 'Join their circle'}<ArrowUpRight size={15} /></button
        ><small
          >{page.data.integrations?.payments === 'sandbox'
            ? 'Sandbox payment · No real charges'
            : page.data.integrations?.payments === 'stripe'
              ? 'Hosted payment through Stripe'
              : 'Review the payment provider before checkout'}</small
        >
      </div>
      <span class="media-label"
        ><LockKeyhole size={11} />{post.visibility === 'purchase'
          ? 'Paid post'
          : 'Subscribers only'}</span
      >{:else if post.media_type !== 'video'}<span class="media-label"
        >{creator.subscribed ? 'In your circle' : 'A glimpse into their world'}</span
      >{/if}
  </div>
  <div class="post-actions">
    <button
      class={['action', { liked }]}
      onclick={onlike}
      aria-label={`${liked ? 'Unlike' : 'Like'} ${creator.display_name}'s post`}
      aria-pressed={liked}
      ><Heart size={21} fill={liked ? 'currentColor' : 'none'} /><span
        >{new Intl.NumberFormat('en-US', { notation: 'compact' }).format(post.like_count)}</span
      ></button
    ><button
      class="action"
      onclick={loadComments}
      aria-label="Show comments"
      aria-expanded={commentsOpen}
      ><MessageCircle size={21} /><span>{post.comment_count}</span></button
    ><button class="action" onclick={share} aria-label="Copy post link"
      >{#if copied}<Check size={19} />{:else}<Send size={19} />{/if}</button
    ><span class="share-status" aria-live="polite"
      >{shareError || (copied ? 'Link copied' : '')}</span
    ><button
      class="action"
      onclick={() => (reportOpen = !reportOpen)}
      aria-label="Report post"
      aria-expanded={reportOpen}><Flag size={18} /></button
    ><button
      class={['action', 'save', { saved }]}
      onclick={onsave}
      aria-label={`${saved ? 'Unsave' : 'Save'} post`}
      aria-pressed={saved}><Bookmark size={21} fill={saved ? 'currentColor' : 'none'} /></button
    >
  </div>
  {#if reportSent}<p class="report-status" role="status">
      Your report has been submitted for review.
    </p>{/if}
  {#if error}<p class="error-message" role="alert">{error}</p>{/if}
  {#if reportOpen}<form class="report-form" onsubmit={report}>
      <label for={`report-${post.id}`}>Tell us what needs review</label><textarea
        id={`report-${post.id}`}
        bind:value={reportReason}
        required
        minlength="10"
        maxlength="2000"
        placeholder="Describe the issue…"></textarea><button
        class="button-secondary"
        disabled={busy || reportReason.trim().length < 10}>Submit report</button
      >
    </form>{/if}
  {#if commentsOpen}<div class="comments" aria-busy={loadingComments}>
      <LoadOlder
        cursor={nextCommentCursor}
        loading={loadingComments || busy}
        label="Older comments"
        onload={fetchComments}
      />
      {#if loadingComments}<p role="status">Loading comments…</p>{/if}
      {#each comments as entry (entry.id)}<p class="local-comment">
          <strong>{entry.display_name}</strong>{entry.body}
        </p>{/each}{#if !comments.length}<p class="comment-empty">Start the conversation.</p>{/if}
      <form onsubmit={addComment}>
        <label class="sr-only" for={`comment-${post.id}`}>Write a comment</label><input
          id={`comment-${post.id}`}
          bind:value={comment}
          maxlength="500"
          placeholder="Leave a little love…"
        /><button
          class="icon-button"
          type="submit"
          disabled={busy || !comment.trim()}
          aria-label="Add comment"><ArrowUpRight size={19} /></button
        >
      </form>
    </div>{/if}
</article>

<style>
  .post {
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 17px;
    overflow: hidden;
  }
  .post-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    padding: 23px 24px 0;
  }
  .creator {
    display: flex;
    gap: 11px;
    text-align: left;
    align-items: center;
  }
  .avatar {
    width: 43px;
    height: 43px;
    object-fit: cover;
    border-radius: 50%;
  }
  .avatar-fallback {
    display: grid;
    place-items: center;
    background: #e7ecda;
    font-size: 18px;
  }
  .creator strong {
    font-size: 13px;
    display: flex;
    align-items: center;
    gap: 5px;
  }
  .creator small {
    display: block;
    color: var(--muted);
    font-size: 10px;
    margin-top: 5px;
  }
  .creator small span {
    margin: 0 5px;
  }
  .category {
    font-size: 9px;
    border-radius: 5px;
    background: #f0f1e9;
    color: #666f5a;
    padding: 6px 8px;
    white-space: nowrap;
  }
  .caption {
    margin: 17px 24px 19px;
    font-size: 12px;
    line-height: 1.85;
    color: #565e53;
    white-space: pre-line;
  }
  .post-media {
    margin: 0 14px;
    border-radius: 10px;
    position: relative;
    overflow: hidden;
    aspect-ratio: 1.58;
    background: #d6d9ce;
  }
  .post-media > img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .media-label {
    position: absolute;
    bottom: 16px;
    left: 16px;
    display: flex;
    align-items: center;
    gap: 5px;
    color: white;
    background: #223e32;
    backdrop-filter: blur(12px);
    font-size: 8px;
    letter-spacing: 0.3px;
    border-radius: 5px;
    padding: 6px 9px;
  }
  .locked::after {
    content: '';
    position: absolute;
    inset: 0;
    background: #20312760;
  }
  .locked > img {
    filter: blur(6px) brightness(0.73);
    transform: scale(1.04);
  }
  .locked .media-label {
    z-index: 2;
  }
  .locked-content {
    position: absolute;
    inset: 0;
    z-index: 2;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    color: white;
    text-align: center;
    padding: 20px;
  }
  .lock-circle {
    width: 52px;
    height: 52px;
    border: 1px solid #ffffff60;
    background: #ffffff17;
    backdrop-filter: blur(5px);
    border-radius: 50%;
    display: grid;
    place-items: center;
    margin-bottom: 15px;
  }
  .locked-content strong {
    font-size: 21px;
    letter-spacing: -0.5px;
  }
  .locked-content p {
    margin: 8px 0 18px;
    font-size: 11px;
    color: #ffffffbd;
  }
  .locked-content small {
    font-size: 8px;
    margin-top: 11px;
    color: #ffffffb5;
  }
  .button-light {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 13px 16px;
    border-radius: 7px;
    background: #f5f6ee;
    color: #273227;
    font-size: 10px;
    font-weight: 600;
  }
  .post-actions {
    display: flex;
    align-items: center;
    gap: 23px;
    padding: 18px 25px 21px;
  }
  .action {
    display: flex;
    align-items: center;
    gap: 7px;
    color: #6d7569;
  }
  .action span {
    font-size: 10px;
  }
  .liked {
    color: #a94968;
  }
  .saved {
    color: #35482b;
  }
  .save {
    margin-left: auto;
  }
  .share-status {
    font-size: 9px;
    color: var(--muted);
  }
  .media-fallback {
    width: 100%;
    height: 100%;
    display: grid;
    place-items: center;
    padding: 20px;
    font-family: Georgia, serif;
    font-style: italic;
    font-size: 24px;
    color: #66755e;
  }
  .comments {
    border-top: 1px solid var(--border);
    padding: 16px 24px;
  }
  .comment-empty {
    font-size: 10px;
    color: var(--muted);
    margin-bottom: 12px;
  }
  .local-comment {
    font-size: 12px;
    margin: 10px 0;
    overflow-wrap: anywhere;
  }
  .local-comment strong {
    margin-right: 7px;
  }
  .comments form {
    display: flex;
    gap: 8px;
  }
  input {
    width: 100%;
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 10px 12px;
    font-size: 11px;
    background: var(--background);
  }
  @media (max-width: 760px) {
    .post-header {
      padding: 18px 17px 0;
    }
    .caption {
      margin: 14px 17px 17px;
      font-size: 12px;
    }
    .post-media {
      margin: 0 9px;
    }
    .post-actions {
      padding: 17px 20px;
      gap: 18px;
    }
    .locked-content strong {
      font-size: 18px;
    }
    .lock-circle {
      width: 42px;
      height: 42px;
      margin-bottom: 10px;
    }
    .locked-content p {
      margin-bottom: 13px;
    }
    .button-light {
      font-size: 9px;
      padding: 11px;
    }
    .locked .media-label {
      display: none;
    }
  }
  .report-form {
    display: grid;
    gap: 12px;
    padding: 0 24px 22px;
    font-size: 12px;
  }
  .report-form textarea {
    padding: 12px;
    background: var(--background);
    border: 1px solid var(--border);
    border-radius: 8px;
    min-height: 80px;
    resize: vertical;
  }
  .report-status {
    padding: 0 24px 18px;
    font-size: 12px;
    color: #4b6438;
  }
</style>
