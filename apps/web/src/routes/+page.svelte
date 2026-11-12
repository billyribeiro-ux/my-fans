<script lang="ts">
  import {
    ArrowUpRight,
    Search,
    Plus,
    Heart,
    Sparkles,
    Users,
    Check,
    ArrowRight,
    RefreshCw,
    Compass,
    Bookmark,
    ShieldCheck,
  } from '@lucide/svelte';
  import { goto, refreshAll } from '$app/navigation';
  import { page, navigating } from '$app/state';
  import { onMount } from 'svelte';
  import PostCard from '#lib/components/PostCard.svelte';
  import CreatorRail from '#lib/components/CreatorRail.svelte';
  import Dialog from '#lib/components/Dialog.svelte';
  import Checkout from '#lib/components/Checkout.svelte';
  import type { FeedCreator, FeedPost } from '#lib/contracts.ts';
  import type { PageProps } from './$types';
  let { data }: PageProps = $props();
  const tabs = [
    { id: 'following', label: 'Following' },
    { id: 'discover', label: 'Discover' },
    { id: 'saved', label: 'Saved' },
  ] as const;
  let tab = $derived(
    ['following', 'saved'].includes(page.url.searchParams.get('tab') ?? '')
      ? page.url.searchParams.get('tab')!
      : 'discover',
  );
  let search = $derived(page.url.searchParams.get('q') ?? '');
  let category = $derived(page.url.searchParams.get('category') ?? 'All');
  let creatorFilter = $derived(page.url.searchParams.get('creator') ?? '');
  let singlePost = $derived(page.url.searchParams.has('post'));
  let hasFilters = $derived(Boolean(search || category !== 'All' || creatorFilter));
  let olderPage = $derived(page.url.searchParams.has('cursor'));
  const categories = ['All', 'Art', 'Travel', 'Fitness', 'Music', 'Food', 'Lifestyle'];
  let selectedId = $state<string | null>(null);
  let checkout = $state<{
    creator: FeedCreator;
    kind: 'subscription' | 'purchase' | 'tip';
    postId?: string;
    amount: number;
  } | null>(null);
  let tipAmount = $state(5);
  let flash = $state('');
  let error = $state('');
  let refreshing = $state(false);
  let disposed = false;
  const mutations = new AbortController();
  onMount(() => () => {
    disposed = true;
    mutations.abort();
  });
  let creators = $derived(data.feed.creators);
  let selectedCreator = $derived(creators.find((c) => c.id === selectedId));
  let heroImage = $derived(creators[0]?.cover_url);
  let matchingPosts = $derived(data.feed.posts);
  function feedHref(changes: Record<string, string | null> = {}) {
    const params = new URLSearchParams();
    for (const key of ['q', 'category', 'tab', 'creator']) {
      const value = page.url.searchParams.get(key);
      if (value) params.set(key, value);
    }
    for (const [key, value] of Object.entries(changes)) {
      if (value) params.set(key, value);
      else params.delete(key);
    }
    return `/${params.size ? `?${params}` : ''}#feed-panel`;
  }
  function searchFeed(event: SubmitEvent) {
    event.preventDefault();
    error = '';
    const fields = new FormData(event.currentTarget as HTMLFormElement);
    void goto(
      feedHref({
        q: String(fields.get('q') ?? '').trim(),
        category: fields.get('category') === 'All' ? null : String(fields.get('category') ?? ''),
        creator: String(fields.get('creator') ?? ''),
      }),
    ).catch(() => {
      error = 'Unable to search the feed. Please try again.';
    });
  }
  const money = (amount: number, currency = 'USD') =>
    new Intl.NumberFormat('en-US', { style: 'currency', currency }).format(amount / 100);
  function selectTab(id: string) {
    void goto(feedHref({ tab: id === 'discover' ? null : id }), { reset: false });
  }
  function authenticated() {
    if (!data.feed.viewer.id) {
      void goto('/auth');
      return false;
    }
    return true;
  }
  function openCheckout(
    creator: FeedCreator,
    kind: 'subscription' | 'purchase' | 'tip' = 'subscription',
    post?: FeedPost,
  ) {
    if (!authenticated()) return;
    selectedId = null;
    checkout = {
      creator,
      kind,
      postId: kind === 'purchase' ? post?.id : undefined,
      amount:
        kind === 'subscription'
          ? creator.subscription_price_minor
          : kind === 'tip'
            ? Math.round(tipAmount * 100)
            : (post?.price_minor ?? 0),
    };
  }
  async function react(post: FeedPost, kind: 'like' | 'save') {
    if (disposed || !authenticated()) return;
    const viewer = data.feed.viewer.id;
    error = '';
    try {
      const response = await fetch(`/api/platform/posts/${post.id}/${kind}`, {
        method: (kind === 'like' ? post.liked : post.saved) ? 'DELETE' : 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
        signal: mutations.signal,
      });
      const result = await response.json();
      if (disposed || viewer !== data.feed.viewer.id) return;
      if (!response.ok) throw new Error(result.message ?? 'Unable to update post.');
      await refreshAll();
    } catch (e) {
      if (!disposed && viewer === data.feed.viewer.id)
        error = e instanceof Error ? e.message : 'Unable to update post.';
    }
  }
  async function refresh() {
    if (disposed || refreshing) return;
    refreshing = true;
    try {
      await refreshAll();
    } catch {
      if (!disposed) error = 'Unable to refresh the feed. Please try again.';
    } finally {
      if (!disposed) refreshing = false;
    }
  }
  function handleTabKey(event: KeyboardEvent) {
    const index = tabs.findIndex((item) => item.id === tab);
    let next = index;
    if (event.key === 'ArrowRight') next = (index + 1) % tabs.length;
    else if (event.key === 'ArrowLeft') next = (index - 1 + tabs.length) % tabs.length;
    else if (event.key === 'Home') next = 0;
    else if (event.key === 'End') next = tabs.length - 1;
    else return;
    event.preventDefault();
    selectTab(tabs[next].id);
    document.getElementById(`tab-${tabs[next].id}`)?.focus();
  }
</script>

<svelte:head
  ><title>MyFans — A little closer to what you love</title><meta
    name="description"
    content="Discover independent creators, support their work, and be part of their story."
  /></svelte:head
>
<div class="page-heading">
  <h1>Your daily dose of inspiration.</h1>
  <p>A little closer to the people you love.</p>
</div>
<div class="columns">
  <main id="main-content">
    {#if page.url.searchParams.get('checkout') === 'canceled'}<aside
        class="checkout-return-note"
        aria-label="Checkout return"
      >
        You left checkout. Check your <a href="/settings">account payment history</a> before retrying
        if you are unsure whether a payment was completed.
      </aside>{/if}
    {#if singlePost}<aside class="checkout-return-note" aria-label="Shared post">
        Viewing a shared post. Access follows your account’s current permissions.
        <a href="/#feed-panel">Back to the feed</a>
      </aside>{/if}
    <section class="hero" aria-labelledby="hero-title">
      <div class="hero-copy">
        <div class="hero-eyebrow"><span></span>LESS SCROLLING. MORE CONNECTION.</div>
        <h2 id="hero-title">A little closer to<br />what you <em>love.</em></h2>
        <p>Discover the people behind the passion.<br />And be part of their story.</p>
        <a class="button-primary" href="/creators">Find your people <ArrowUpRight size={16} /></a>
      </div>
      <div class="hero-art">
        {#if heroImage}<img
            src={heroImage}
            alt="A glimpse into a creator’s world"
            width="450"
            height="500"
            fetchpriority="high"
          />{:else}<div class="hero-art-fallback"><Sparkles size={50} /></div>{/if}<span
          class="hero-art-orbit"
        ></span>
        <div class="hero-sticker">
          <Heart size={16} fill="currentColor" /><span>CREATOR FIRST.<br />ALWAYS.</span>
        </div>
      </div>
      <span class="hero-star"><Sparkles size={24} /></span>
    </section>
    {#if creators.length}<section class="stories" aria-label="Meet the creators">
        <a class="your-story" href="/studio"
          ><span class="add-story"><Plus size={24} /></span><small>Your story</small></a
        >{#each creators.slice(0, 6) as creator (creator.id)}<button
            class="story"
            onclick={() => (selectedId = creator.id)}
            aria-label={`View ${creator.display_name}'s profile`}
            ><span class={['story-ring', { subscribed: creator.subscribed }]}
              >{#if creator.avatar_url}<img
                  src={creator.avatar_url}
                  alt=""
                  width="61"
                  height="61"
                />{:else}<span class="story-initial" aria-hidden="true"
                  >{creator.display_name.charAt(0)}</span
                >{/if}</span
            ><small>{creator.display_name.split(' ')[0]}</small></button
          >{/each}
      </section>{/if}
    <div class="feed-toolbar">
      <div class="feed-tabs" role="tablist" aria-label="Feed selection">
        {#each tabs as item (item.id)}<button
            role="tab"
            id={`tab-${item.id}`}
            aria-selected={tab === item.id}
            aria-controls="feed-panel"
            tabindex={tab === item.id ? 0 : -1}
            class={{ active: tab === item.id }}
            onclick={() => selectTab(item.id)}
            onkeydown={handleTabKey}>{item.label}</button
          >{/each}
      </div>
    </div>
    <form
      class="feed-search"
      action="/"
      method="GET"
      role="search"
      aria-label="Search creator posts"
      onsubmit={searchFeed}
    >
      {#if tab !== 'discover'}<input type="hidden" name="tab" value={tab} />{/if}
      <label class="search"
        ><Search size={16} /><span class="sr-only">Search this feed</span><input
          name="q"
          value={search}
          placeholder="Search creator or caption"
          type="search"
          maxlength="100"
        /></label
      >
      <label class="filter-select"
        ><span class="sr-only">Creator category</span><select name="category" value={category}
          >{#each categories as item (item)}<option value={item}
              >{item === 'All' ? 'All categories' : item}</option
            >{/each}</select
        ></label
      >
      <label class="filter-select"
        ><span class="sr-only">Filter by creator</span><select name="creator" value={creatorFilter}
          ><option value="">All creators</option
          >{#if creatorFilter && !creators.some((creator) => creator.id === creatorFilter)}<option
              value={creatorFilter}>Selected creator</option
            >{/if}{#each creators as creator (creator.id)}<option value={creator.id}
              >{creator.display_name}</option
            >{/each}</select
        ></label
      >
      <button class="button-secondary" disabled={Boolean(navigating.to)}>Search posts</button>
      {#if hasFilters}<a
          class="text-button"
          href={feedHref({ q: null, category: null, creator: null })}>Clear filters</a
        >{/if}
    </form>
    {#if hasFilters}<p class="feed-results" role="status">
        {matchingPosts.length}
        {matchingPosts.length === 1 ? 'post' : 'posts'} on this page{search
          ? ` matching “${search}”`
          : ''}{category !== 'All' ? ` · ${category}` : ''}{creatorFilter
          ? ` · ${creators.find((creator) => creator.id === creatorFilter)?.display_name ?? 'Selected creator'}`
          : ''}.
      </p>{/if}
    {#if flash}<div class="status-message" role="status">{flash}</div>{/if}{#if error}<div
        class="error-message"
        role="alert"
      >
        {error}
      </div>{/if}
    {#if data.error}<div class="error-state" role="alert">
        <ShieldCheck size={27} />
        <h2>Your space will be right back.</h2>
        <p>{data.error}</p>
        <button class="button-secondary" onclick={refresh} disabled={refreshing}
          ><RefreshCw size={15} />{refreshing ? 'Reconnecting…' : 'Try again'}</button
        >
      </div>{/if}
    <div class="feed" id="feed-panel" role="tabpanel" aria-labelledby={`tab-${tab}`} tabindex="0">
      {#each matchingPosts as post (`${data.feed.viewer.id}:${post.id}`)}{@const creator =
          creators.find((c) => c.id === post.creator_id)}{#if creator}<PostCard
            {post}
            {creator}
            liked={post.liked}
            saved={post.saved}
            onlike={() => react(post, 'like')}
            onsave={() => react(post, 'save')}
            onprofile={() => (selectedId = creator.id)}
            onsubscribe={() =>
              openCheckout(
                creator,
                post.visibility === 'purchase' ? 'purchase' : 'subscription',
                post,
              )}
            authenticated={Boolean(data.feed.viewer.id)}
          />{/if}{/each}
      {#if matchingPosts.length === 0 && !data.error}<div class="empty-state">
          {#if singlePost}<Compass size={28} />
            <h2>This post is unavailable.</h2>
            <p>The post may have been removed or may be unavailable to this account.</p>
          {:else if tab === 'saved'}<Bookmark size={28} />
            <h2>Inspiration for later.</h2>
            <p>
              {hasFilters
                ? 'No saved posts match these filters.'
                : 'Bookmark a post to save it to your account.'}
            </p>{:else}<Compass size={28} />
            <h2>{hasFilters ? 'Keep exploring.' : 'Your circle starts here.'}</h2>
            <p>
              {hasFilters
                ? 'Try a creator name or a different interest.'
                : 'Discover a creator and join their circle.'}
            </p>{/if}<a class="button-secondary" href="/creators"
            >Explore creators <ArrowRight size={15} /></a
          >
        </div>{/if}
    </div>
    {#if !data.error}<nav class="feed-pagination" aria-label="Feed pages">
        {#if olderPage}<a class="button-secondary" href={feedHref()}>Back to newest posts</a>{/if}
        {#if data.feed.next_cursor}<a
            class="button-primary"
            href={feedHref({ cursor: data.feed.next_cursor })}
            rel="next">Older posts <ArrowRight size={15} /></a
          >{:else if matchingPosts.length > 0}<p class="feed-end">
            <span></span><Heart size={13} />You’re all caught up.<span></span>
          </p>{/if}
      </nav>{/if}
  </main>
  <CreatorRail
    {creators}
    onprofile={(creator) => (selectedId = creator.id)}
    onsubscribe={(creator) => openCheckout(creator)}
  />
</div>
{#if selectedCreator}<Dialog title="Meet your next inspiration" onclose={() => (selectedId = null)}
    ><div class="profile-cover">
      {#if selectedCreator.cover_url}<img
          src={selectedCreator.cover_url}
          alt=""
          width="600"
          height="220"
        />{/if}
    </div>
    {#if selectedCreator.avatar_url}<img
        class="profile-avatar"
        src={selectedCreator.avatar_url}
        alt=""
        width="76"
        height="76"
      />{:else}<span class="profile-avatar avatar-initial" aria-hidden="true"
        >{selectedCreator.display_name.charAt(0)}</span
      >{/if}
    <h3 class="profile-name">{selectedCreator.display_name}</h3>
    <p class="profile-handle">@{selectedCreator.handle} · {selectedCreator.category}</p>
    <p class="profile-bio">{selectedCreator.bio}</p>
    <div class="profile-stats">
      <Users size={17} />{selectedCreator.subscriber_count} in their community
    </div>
    {#if selectedCreator.subscribed}<div class="subscribed-note">
        <Check size={16} />You’re in their circle
      </div>{:else}<button
        class="button-primary full-width"
        onclick={() => selectedCreator && openCheckout(selectedCreator)}
        >Join their circle · {money(
          selectedCreator.subscription_price_minor,
          selectedCreator.currency,
        )}/month <ArrowUpRight size={16} /></button
      >{/if}
    <div class="profile-actions">
      <a
        class="button-secondary"
        href={feedHref({ creator: selectedCreator.id })}
        onclick={() => (selectedId = null)}>View creator posts</a
      >
      <a class="button-secondary" href={`/messages?recipient=${selectedCreator.user_id}`}
        >Message creator</a
      ><label class="tip-label"
        >Tip in USD<input
          aria-label="Tip amount in USD"
          type="number"
          min="1"
          max="10000"
          step="1"
          bind:value={tipAmount}
        /></label
      ><button
        class="button-secondary"
        disabled={!tipAmount || tipAmount < 1}
        onclick={() => selectedCreator && openCheckout(selectedCreator, 'tip')}
        >{data.integrations?.payments === 'sandbox' ? 'Send sandbox tip' : 'Send a tip'}</button
      >
    </div>
    <p class="modal-footnote">
      {data.integrations?.payments === 'sandbox'
        ? 'Sandbox environment. No real money is charged.'
        : data.integrations?.payments === 'stripe'
          ? 'Payment is completed through hosted Stripe Checkout.'
          : 'Check the payment provider before checkout.'}
    </p></Dialog
  >{/if}
{#if checkout && data.feed.viewer.id}{#key `${data.feed.viewer.id}:${checkout.creator.id}:${checkout.kind}:${checkout.postId ?? ''}`}<Checkout
      creatorId={checkout.creator.id}
      creatorName={checkout.creator.display_name}
      amountMinor={checkout.amount}
      kind={checkout.kind}
      postId={checkout.postId}
      onclose={() => (checkout = null)}
      oncomplete={() => {
        flash = 'Sandbox payment confirmed. Your account has been updated.';
        void refreshAll();
      }}
    />{/key}{/if}

<style>
  .checkout-return-note {
    padding: 18px;
    margin-bottom: 22px;
    border: 1px solid var(--border);
    border-radius: 12px;
    font-size: 13px;
    line-height: 1.8;
  }
  .checkout-return-note a {
    text-decoration: underline;
  }
  .columns {
    display: flex;
    align-items: flex-start;
    gap: 30px;
  }
  main {
    flex: 1;
    min-width: 0;
    max-width: 720px;
  }
  .hero {
    min-height: 284px;
    border-radius: 17px;
    background: #e7ecda;
    position: relative;
    overflow: hidden;
  }
  .hero-copy {
    position: relative;
    z-index: 2;
    padding: 29px 27px;
    width: 66%;
  }
  .hero-eyebrow {
    font-size: 7px;
    font-weight: 700;
    letter-spacing: 1.5px;
    display: flex;
    align-items: center;
    gap: 7px;
    color: #59674a;
  }
  .hero-eyebrow > span {
    width: 4px;
    height: 4px;
    border-radius: 50%;
    background: #778b5f;
  }
  .hero h2 {
    font-family: 'Manrope', sans-serif;
    font-size: clamp(27px, 2.8vw, 39px);
    line-height: 1.2;
    letter-spacing: -1.6px;
    font-weight: 700;
    margin-top: 20px;
  }
  .hero em {
    font-family: Georgia, serif;
    font-weight: 400;
    color: #647c51;
  }
  .hero p {
    font-size: 10px;
    line-height: 1.8;
    margin: 15px 0 19px;
    color: #59674a;
  }
  .hero .button-primary {
    padding: 11px 14px;
    font-size: 10px;
  }
  .hero-art {
    position: absolute;
    width: 39%;
    height: 88%;
    right: 5%;
    top: 6%;
    border-radius: 130px 130px 130px 130px;
    transform: rotate(13deg);
  }
  .hero-art > img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    border-radius: inherit;
    filter: saturate(0.65);
  }
  .hero-art-fallback {
    display: grid;
    place-items: center;
    width: 100%;
    height: 100%;
    background: #b7c0a5;
    border-radius: inherit;
    color: #f6f7ed;
  }
  .hero-art-orbit {
    position: absolute;
    inset: -10px;
    border: 1px solid #bec7aa;
    border-radius: inherit;
  }
  .hero-sticker {
    position: absolute;
    right: -17px;
    bottom: 29px;
    width: 81px;
    height: 81px;
    border-radius: 50%;
    background: #edd6d4;
    display: flex;
    justify-content: center;
    align-items: center;
    flex-direction: column;
    gap: 5px;
    color: #704a46;
    transform: rotate(-26deg);
    border: 4px solid #e7ecda;
  }
  .hero-sticker span {
    font-size: 6px;
    font-weight: 700;
    line-height: 1.5;
    letter-spacing: 1px;
    text-align: center;
  }
  .hero-star {
    position: absolute;
    right: 12px;
    top: 19px;
    color: #91a479;
  }
  .stories {
    display: flex;
    gap: 23px;
    overflow-x: auto;
    padding: 27px 3px 25px;
    scrollbar-width: none;
  }
  .story,
  .your-story {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 9px;
    flex-shrink: 0;
  }
  .story-ring {
    padding: 3px;
    display: block;
    width: 65px;
    height: 65px;
    border: 1px solid #d3b9b7;
    border-radius: 50%;
  }
  .story-ring.subscribed {
    border-color: #98a887;
  }
  .story-initial {
    display: grid;
    place-items: center;
    width: 100%;
    height: 100%;
    border-radius: 50%;
    background: #e7ecda;
    font-size: 25px;
  }
  .avatar-initial {
    display: inline-grid;
    place-items: center;
    background: #e7ecda;
    font-size: 30px;
  }
  .story-ring img {
    object-fit: cover;
    width: 100%;
    height: 100%;
    border-radius: 50%;
  }
  .story small,
  .your-story small {
    font-size: 9px;
    color: #687360;
  }
  .add-story {
    display: grid;
    place-items: center;
    width: 65px;
    height: 65px;
    border-radius: 50%;
    border: 1px dashed #c5ccba;
    color: #8d9b7b;
    background: #f0f2e9;
  }
  .feed-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    border-bottom: 1px solid var(--border);
    margin-bottom: 21px;
  }
  .feed-tabs {
    display: flex;
    align-items: center;
    gap: 24px;
  }
  .feed-tabs button {
    position: relative;
    height: 44px;
    color: #687360;
    font-size: 11px;
    font-weight: 600;
    display: flex;
    align-items: center;
    gap: 7px;
  }
  .feed-tabs button.active {
    color: #3f5035;
  }
  .feed-tabs button.active::after {
    content: '';
    position: absolute;
    bottom: -1px;
    left: 0;
    right: 0;
    height: 2px;
    background: #778d62;
  }
  .search {
    display: flex;
    align-items: center;
    gap: 8px;
    color: #a4ac9a;
  }
  .search input {
    width: 100%;
    background: transparent;
    border: 0;
    min-width: 0;
    outline-offset: 2px;
    font-size: 12px;
    padding: 10px 0;
  }
  .feed-search {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 10px;
    margin: 0 0 22px;
  }
  .feed-search .search {
    flex: 1 1 230px;
    border: 1px solid var(--border);
    border-radius: 9px;
    padding: 0 11px;
    background: var(--surface);
    min-width: 0;
  }
  .filter-select {
    flex: 1 1 125px;
    min-width: 0;
  }
  .filter-select select {
    width: 100%;
    padding: 11px 8px;
    border: 1px solid var(--border);
    border-radius: 9px;
    font: inherit;
    font-size: 11px;
    background: var(--surface);
    color: var(--ink);
  }
  .feed-results {
    margin: -7px 0 18px;
    font-size: 12px;
    color: var(--muted);
    overflow-wrap: anywhere;
  }
  .feed-pagination {
    display: flex;
    align-items: center;
    justify-content: space-between;
    flex-wrap: wrap;
    gap: 15px;
    margin: 25px 0 40px;
  }
  .feed-pagination .feed-end {
    flex: 1;
    margin: 0;
  }
  .feed {
    display: grid;
    gap: 23px;
  }
  .feed:focus-visible {
    outline-offset: 6px;
  }
  .feed-end {
    display: flex;
    align-items: center;
    gap: 10px;
    margin: 30px 0 44px;
    color: #687360;
    font-size: 9px;
  }
  .feed-end > span {
    flex: 1;
    height: 1px;
    background: var(--border);
  }
  .empty-state,
  .error-state {
    padding: 42px 25px;
    border: 1px solid var(--border);
    border-radius: 15px;
    background: var(--surface);
    text-align: center;
  }
  .empty-state > :global(svg),
  .error-state > :global(svg) {
    color: #8a9b74;
    margin-bottom: 15px;
  }
  .empty-state h2,
  .error-state h2 {
    font-size: 20px;
    letter-spacing: -0.4px;
  }
  .empty-state p,
  .error-state p {
    font-size: 12px;
    color: var(--muted);
    line-height: 1.8;
    max-width: 350px;
    margin: 12px auto 22px;
  }
  .error-state {
    margin-bottom: 20px;
  }
  .profile-cover {
    background: linear-gradient(130deg, #d7e1c8, #eee3da);
    height: 140px;
    border-radius: 12px;
    overflow: hidden;
  }
  .profile-cover img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }
  .profile-avatar {
    border-radius: 50%;
    object-fit: cover;
    width: 76px;
    height: 76px;
    border: 4px solid var(--surface);
    position: relative;
    margin: -37px 0 10px 15px;
  }
  .profile-name {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 24px;
    letter-spacing: -0.7px;
  }
  .profile-handle {
    font-size: 11px;
    color: var(--muted);
    margin-top: 7px;
  }
  .profile-bio {
    font-size: 13px;
    color: #606a58;
    line-height: 1.85;
    margin: 20px 0;
  }
  .profile-stats {
    display: flex;
    gap: 8px;
    align-items: center;
    color: #657457;
    font-size: 11px;
    margin-bottom: 23px;
  }
  .full-width {
    width: 100%;
  }
  .modal-footnote {
    text-align: center;
    font-size: 10px;
    color: var(--muted);
    line-height: 1.7;
    margin-top: 13px;
  }
  .subscribed-note {
    display: flex;
    gap: 8px;
    align-items: center;
    justify-content: center;
    background: #eef2e7;
    padding: 15px;
    border-radius: 8px;
    color: #627b49;
    font-size: 11px;
  }

  .profile-actions {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
    align-items: end;
    margin-top: 18px;
  }
  .tip-label {
    font-size: 11px;
    display: grid;
    gap: 6px;
  }
  .tip-label input {
    width: 85px;
    padding: 9px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--background);
  }
  @media (max-width: 760px) {
    .columns {
      display: block;
    }
    .hero {
      min-height: 260px;
    }
    .hero-copy {
      padding: 24px 20px;
      width: 72%;
    }
    .hero h2 {
      font-size: 29px;
    }
    .hero-art {
      right: -3%;
      width: 37%;
    }
    .hero-sticker {
      right: 0;
      bottom: 15px;
      width: 65px;
      height: 65px;
    }
    .stories {
      gap: 19px;
    }
    .feed-tabs {
      gap: 18px;
    }
    .search input {
      width: 100%;
    }
    .profile-name {
      font-size: 21px;
    }
  }
</style>
