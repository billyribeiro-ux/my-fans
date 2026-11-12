<script lang="ts">
  import { onMount } from 'svelte';
  import MediaCaption from '#lib/components/MediaCaption.svelte';
  import CollectionPages from '#lib/components/CollectionPages.svelte';
  import IntegrationSetup from '#lib/components/IntegrationSetup.svelte';
  import CreatorOnboarding from '#lib/components/CreatorOnboarding.svelte';
  import ResumableUpload from '#lib/components/ResumableUpload.svelte';
  import PrivateVideo from '#lib/components/PrivateVideo.svelte';
  import { goto, refreshAll } from '$app/navigation';
  import { page } from '$app/state';
  import { Film, Clock, ArrowUpRight, ShieldCheck, RefreshCw } from '@lucide/svelte';
  import type { PageProps } from './$types';
  import type { StudioPost as Post } from '#lib/route-data.ts';
  let { data }: PageProps = $props();
  let disposed = false;
  const controller = new AbortController();
  onMount(() => () => {
    disposed = true;
    controller.abort();
  });
  function currentAccount(owner: StudioDraft) {
    return !disposed && owner === draft;
  }
  let creator = $derived(data.studio?.creator);
  let posts = $derived(data.studio?.posts ?? []);
  let media = $derived(data.studio?.media ?? []);
  interface ProfileDraft {
    handle?: string;
    name?: string;
    bio?: string;
    category?: string;
    monthlyPrice?: number;
  }
  let profileDraft = $state.raw<{ userId: string; fields: ProfileDraft } | null>(null);
  let profileFields = $derived(
    profileDraft && profileDraft.userId === data.user?.id ? profileDraft.fields : {},
  );
  let handle = $derived(profileFields.handle ?? creator?.handle ?? '');
  let name = $derived(profileFields.name ?? creator?.display_name ?? '');
  let bio = $derived(profileFields.bio ?? creator?.bio ?? '');
  let category = $derived(profileFields.category ?? creator?.category ?? 'Art');
  let monthlyPrice = $derived(
    'monthlyPrice' in profileFields
      ? profileFields.monthlyPrice
      : (creator?.subscription_price_minor ?? 1000) / 100,
  );
  function editProfile<K extends keyof ProfileDraft>(field: K, value: ProfileDraft[K]) {
    if (data.user)
      profileDraft = {
        userId: data.user.id,
        fields: { ...profileFields, [field]: value },
      };
  }
  class StudioDraft {
    caption = $state('');
    visibility = $state('public');
    price = $state(5);
    mediaId = $state('');
    scheduled = $state('');
    busy = $state(false);
    error = $state('');
    message = $state('');
    editingId = $state<string | null>(null);
    readonly userId: string | undefined;
    constructor(userId: string | undefined) {
      this.userId = userId;
    }
  }
  let ownerId = $derived(data.user?.id);
  let draft = $derived(new StudioDraft(ownerId));
  async function newest(parameter: string, owner = draft) {
    if (!currentAccount(owner)) return false;
    const url = new URL(page.url.href);
    url.searchParams.delete(parameter);
    if (page.url.searchParams.has(parameter))
      await goto(`${url.pathname}${url.search}`, { reset: false });
    return currentAccount(owner);
  }
  async function request(path: string, body: unknown, method = 'POST') {
    const owner = draft;
    const response = await fetch(`/api/platform/${path}`, {
      method,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
      signal: controller.signal,
    });
    const result = await response.json();
    if (!currentAccount(owner)) throw new DOMException('The account view changed.', 'AbortError');
    if (!response.ok)
      throw new Error(result.message ?? result.error ?? 'Unable to save your changes.');
    return result;
  }
  async function act(task: (current: StudioDraft) => Promise<void>, refreshed?: () => void) {
    if (draft.busy) return;
    const owner = draft;
    draft.busy = true;
    draft.error = '';
    draft.message = '';
    try {
      await task(owner);
      if (!currentAccount(owner)) return;
      await refreshAll();
      if (currentAccount(owner)) refreshed?.();
    } catch (e) {
      if (currentAccount(owner))
        draft.error = e instanceof Error ? e.message : 'Unable to complete this action.';
    } finally {
      if (currentAccount(owner)) owner.busy = false;
    }
  }
  function profile(event: SubmitEvent) {
    event.preventDefault();
    if (monthlyPrice === undefined) return;
    const submitted = profileDraft;
    const subscriptionPrice = Math.round(monthlyPrice * 100);
    void act(
      async (current) => {
        await request('creators/profile', {
          handle,
          display_name: name,
          bio,
          category,
          subscription_price_minor: subscriptionPrice,
        });
        if (!currentAccount(current)) return;
        current.message = 'Creator profile saved. Verification requires administrator review.';
      },
      () => {
        if (profileDraft === submitted) profileDraft = null;
      },
    );
  }
  function savePost(state: string) {
    void act(async (current) => {
      const body = current.editingId
        ? { caption: current.caption, state }
        : {
            caption: current.caption,
            visibility: current.visibility,
            price_minor: current.visibility === 'purchase' ? Math.round(current.price * 100) : null,
            media_id: current.mediaId || null,
            state,
            scheduled_at: state === 'scheduled' ? new Date(current.scheduled).toISOString() : null,
          };
      await request(
        current.editingId ? `posts/${current.editingId}` : 'posts',
        body,
        current.editingId ? 'PATCH' : 'POST',
      );
      if (!currentAccount(current)) return;
      current.caption = '';
      current.editingId = null;
      if (!(await newest('posts_cursor', current))) return;
      current.mediaId = '';
      current.scheduled = '';
      current.message =
        state === 'published'
          ? 'Post published.'
          : state === 'scheduled'
            ? 'Post scheduled.'
            : 'Draft saved.';
    });
  }
  function edit(post: Post) {
    draft.editingId = post.id;
    draft.caption = post.caption;
    draft.visibility = post.visibility;
    draft.price = (post.price_minor ?? 500) / 100;
    draft.mediaId = post.media_id ?? '';
    draft.scheduled = post.scheduled_at ? post.scheduled_at.slice(0, 16) : '';
    document.getElementById('post-caption')?.focus();
  }
  function changeState(post: Post, state: string) {
    void act(async (current) => {
      await request(`posts/${post.id}`, { state }, 'PATCH');
      if (!currentAccount(current)) return;
      current.message = state === 'published' ? 'Post published.' : 'Post returned to drafts.';
    });
  }
</script>

<svelte:head><title>Creator studio · MyFans</title></svelte:head>
<main id="main-content">
  <div class="page-heading">
    <h1>Your passion has a place.</h1>
    <p>Create, publish, and grow a space with your people.</p>
    {#if data.user}<a class="text-button studio-earnings" href="/earnings"
        >View earnings and payouts <ArrowUpRight size={14} /></a
      >{/if}
  </div>
  {#if data.error}<p class="error-message" role="alert">{data.error}</p>{/if}{#if draft.error}<p
      class="error-message"
      role="alert"
    >
      {draft.error}
    </p>{/if}{#if draft.message}<p class="status-message" role="status">{draft.message}</p>{/if}
  {#if !data.user}<section class="panel empty-panel">
      <Film size={32} />
      <h2>Your story starts here.</h2>
      <p>Sign in or create an account to set up your creator profile.</p>
      <a class="button-primary" href="/auth">Sign in <ArrowUpRight size={16} /></a>
    </section>{:else}
    <div class="two-column">
      <section class="panel">
        <h2>{creator ? 'Creator profile' : 'Become a creator'}</h2>
        <p class="section-description">
          Your public identity and subscription pricing.
          {data.integrations?.identity === 'sumsub'
            ? 'Hosted identity verification and profile review are separate steps.'
            : data.integrations?.identity === 'sandbox'
              ? 'Sandbox verification is a review status and does not constitute identity verification.'
              : 'Check provider status below before starting identity or payment setup.'}
        </p>
        {#if creator}<p class="review-note">
            <ShieldCheck size={18} />Review:
            <span class="status-pill">{creator.verification_status ?? 'pending'}</span>
          </p>{/if}
        <form class="form-grid" onsubmit={profile}>
          <label
            >Creator name<input
              bind:value={() => name, (value) => editProfile('name', value)}
              required
              minlength="2"
              maxlength="80"
            /></label
          ><label
            >Handle<input
              bind:value={() => handle, (value) => editProfile('handle', value)}
              aria-label="Handle"
              pattern={'[a-z0-9_]{3,30}'}
              required
              minlength="3"
              maxlength="30"
            /><small>3–30 lowercase letters, numbers, or underscores.</small></label
          ><label
            >About you<textarea
              bind:value={() => bio, (value) => editProfile('bio', value)}
              maxlength="2000"
              required></textarea></label
          ><label
            >Category<select bind:value={() => category, (value) => editProfile('category', value)}
              ><option>Art</option><option>Travel</option><option>Fitness</option><option
                >Music</option
              ><option>Food</option><option>Lifestyle</option></select
            ></label
          ><label
            >Monthly subscription in USD<input
              type="number"
              min="1"
              max="1000"
              step="0.01"
              bind:value={() => monthlyPrice, (value) => editProfile('monthlyPrice', value)}
              required
            /><small
              >{data.integrations?.payments === 'stripe'
                ? 'Stripe handles subscription checkout and payment.'
                : data.integrations?.payments === 'sandbox'
                  ? 'Sandbox price. No real money is charged.'
                  : 'Check the payment provider status before publishing paid access.'}</small
            ></label
          ><button class="button-primary" disabled={draft.busy}
            >{draft.busy ? 'Saving…' : 'Save creator profile'}</button
          >
        </form>
      </section>
      <div class="stack">
        {#if creator}{#key data.user.id}<CreatorOnboarding
              userId={data.user.id}
              onboarding={data.studio?.onboarding}
              identityMode={data.integrations?.identity}
              paymentMode={data.integrations?.payments}
            />{/key}{/if}
        {#key data.user.id}
          <IntegrationSetup
            status={data.integrations}
            initialError={data.integrationError}
            allowSetup={!!creator}
          />
        {/key}
        <section class="panel">
          <h2>Media library</h2>
          <p class="section-description">
            Upload an image or video. Private files become publishable after processing and review.
          </p>
          {#key data.user.id}
            <ResumableUpload
              userId={data.user.id}
              disabled={!creator || draft.busy}
              oncomplete={async () => {
                draft.mediaId = '';
                draft.message =
                  'Upload received. Processing, captions for videos, and review are required before publication.';
                await newest('media_cursor');
                await refreshAll();
              }}
            />
          {/key}
          <button
            class="text-button refresh-media"
            onclick={() => refreshAll()}
            disabled={draft.busy}><RefreshCw size={13} />Refresh processing status</button
          >{#each media as item (item.id)}<div class="list-row">
              <span
                ><strong>{item.id.slice(0, 8)}</strong><small
                  >{new Date(item.created_at).toLocaleDateString()}</small
                ></span
              ><span class="status-pill">{item.state}</span>
              {#if item.state === 'approved'}<a
                  class="text-button"
                  href={`/api/platform/media/${item.id}/full`}
                  download={`myfans-${item.id}.${item.content_type?.startsWith('video/') ? 'mp4' : 'jpg'}`}
                  >Download approved {item.content_type?.startsWith('video/')
                    ? 'video'
                    : 'image'}</a
                >{/if}
            </div>
            {#if item.state === 'rejected'}<p class="media-review-feedback">
                <strong>Media review:</strong>
                {item.rejection_reason?.trim() ||
                  'A review reason is unavailable. Contact support with this media reference.'}
                <span
                  >Review the <a href="/legal#content">content policy</a> before uploading a replacement.</span
                >
              </p>{/if}
            {#if item.state === 'approved' && item.content_type?.startsWith('video/')}<div
                class="studio-video"
              >
                <PrivateVideo
                  src={`/api/platform/media/${item.id}/full`}
                  hlsUrl={item.hls_url}
                  poster={item.preview_url}
                  captionsUrl={item.captions_url ??
                    (item.captions_ready ? `/api/platform/media/${item.id}/captions` : null)}
                  label={`Your video ${item.id.slice(0, 8)}`}
                />
              </div>{/if}
            {#if item.content_type?.startsWith('video/') && item.state !== 'rejected'}<MediaCaption
                mediaId={item.id}
                ready={Boolean(item.captions_ready)}
                oncomplete={() => refreshAll()}
              />{/if}{/each}
          <CollectionPages
            nextCursor={data.studio?.next_cursors?.media}
            parameter="media_cursor"
            label="Studio media"
            nextLabel="Older media"
          />
        </section>
        <section class="panel">
          <h2>{draft.editingId ? 'Edit your post' : 'Share something good'}</h2>
          <form
            class="form-grid"
            onsubmit={(event) => {
              event.preventDefault();
              savePost('draft');
            }}
          >
            <label
              >Caption<textarea
                id="post-caption"
                bind:value={draft.caption}
                required
                maxlength="5000"
                placeholder="Let your people into your world…"></textarea></label
            >
            <div class="form-row">
              <label
                >Who can see it?<select
                  bind:value={draft.visibility}
                  disabled={Boolean(draft.editingId)}
                  ><option value="public">Everyone</option><option value="subscribers"
                    >Subscribers</option
                  ><option value="purchase">Paid unlock</option></select
                ></label
              >{#if draft.visibility === 'purchase'}<label
                  >Unlock price in USD<input
                    type="number"
                    min="1"
                    max="10000"
                    step="0.01"
                    bind:value={draft.price}
                    required
                    disabled={Boolean(draft.editingId)}
                  /></label
                >{/if}
            </div>
            <label
              >Attach approved media<select
                bind:value={draft.mediaId}
                disabled={Boolean(draft.editingId)}
                ><option value="">Text-only post</option
                >{#each media.filter((item) => item.state === 'approved' && (!item.content_type?.startsWith('video/') || item.captions_ready)) as item (item.id)}<option
                    value={item.id}
                    >{item.id.slice(0, 8)} · {item.content_type?.startsWith('video/')
                      ? 'video'
                      : 'image'} · approved</option
                  >{/each}</select
              ></label
            ><label
              >Schedule for<input
                type="datetime-local"
                bind:value={draft.scheduled}
                disabled={Boolean(draft.editingId)}
              /><small
                >{draft.editingId
                  ? 'Access, pricing, media, and schedule are fixed after creation. Create a new post to change them.'
                  : 'Uses your local timezone.'}</small
              ></label
            >
            <div class="form-actions">
              <button
                class="button-secondary"
                disabled={draft.busy || !creator || !draft.caption.trim()}>Save draft</button
              ><button
                type="button"
                class="button-primary"
                onclick={() => savePost('published')}
                disabled={draft.busy || !creator || !draft.caption.trim()}>Publish</button
              ><button
                type="button"
                class="button-secondary"
                onclick={() => savePost('scheduled')}
                disabled={draft.busy ||
                  !creator ||
                  !draft.caption.trim() ||
                  !draft.scheduled ||
                  Boolean(draft.editingId)}><Clock size={15} />Schedule</button
              >{#if draft.editingId}<button
                  type="button"
                  class="text-button"
                  onclick={() => {
                    draft.editingId = null;
                    draft.caption = '';
                  }}>Cancel edit</button
                >{/if}
            </div>
          </form>
        </section>
      </div>
    </div>
    <section class="panel studio-posts">
      <h2>Your posts</h2>
      {#if !posts.length}<p>
          Your first story is waiting to be written.
        </p>{/if}{#each posts as post (post.id)}<div class="list-row">
          <div>
            <strong
              >{post.caption.length > 100 ? post.caption.slice(0, 100) + '…' : post.caption}</strong
            ><small
              >{post.visibility}{post.scheduled_at
                ? ` · ${new Date(post.scheduled_at).toLocaleString()}`
                : ''}</small
            >
          </div>
          <div class="post-controls">
            <span class="status-pill">{post.state}</span><button
              class="text-button"
              onclick={() => edit(post)}>Edit</button
            ><button
              class="text-button"
              disabled={draft.busy}
              onclick={() => changeState(post, post.state === 'published' ? 'draft' : 'published')}
              >{post.state === 'published' ? 'Unpublish' : 'Publish'}</button
            >
          </div>
        </div>{/each}
      <CollectionPages
        nextCursor={data.studio?.next_cursors?.posts}
        parameter="posts_cursor"
        label="Studio posts"
        nextLabel="Older posts"
      />
    </section>{/if}
</main>

<style>
  .media-review-feedback {
    margin: 0 0 18px;
    font-size: 12px;
    overflow-wrap: anywhere;
  }
  .media-review-feedback span {
    display: block;
    margin-top: 6px;
  }
  .media-review-feedback a {
    text-decoration: underline;
  }
  .studio-video {
    aspect-ratio: 16 / 9;
    overflow: hidden;
    border-radius: 10px;
    margin: 10px 0;
  }
  .studio-earnings {
    display: inline-flex;
    gap: 6px;
    align-items: center;
    margin-top: 12px;
  }
  .section-description {
    margin-bottom: 20px;
  }
  .review-note {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 18px 0 !important;
  }
  .refresh-media {
    display: flex;
    align-items: center;
    gap: 7px;
    margin-top: 20px;
  }
  .studio-posts {
    margin-top: 22px;
  }
  .post-controls {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-shrink: 0;
  }
  @media (max-width: 900px) {
    .list-row {
      flex-wrap: wrap;
    }
    .post-controls {
      flex-wrap: wrap;
    }
  }
</style>
