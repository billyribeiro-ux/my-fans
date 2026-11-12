<script lang="ts">
  import { page, navigating } from '$app/state';
  import { refreshAll } from '$app/navigation';
  import { Compass, Search, ArrowUpRight, Check, RefreshCw } from '@lucide/svelte';
  import CollectionPages from '#lib/components/CollectionPages.svelte';
  import type { PageProps } from './$types';
  let { data }: PageProps = $props();
  let query = $derived(page.url.searchParams.get('q') ?? '');
  let category = $derived(page.url.searchParams.get('category') ?? '');
  let filtered = $derived(Boolean(query || category));
  let busy = $state(false);
  let error = $state('');
  async function retry() {
    if (busy) return;
    busy = true;
    error = '';
    try {
      await refreshAll();
    } catch {
      error = 'Unable to refresh creators. Please try again.';
    } finally {
      busy = false;
    }
  }
  const money = (amount: number, currency: string) =>
    new Intl.NumberFormat('en-US', { style: 'currency', currency }).format(amount / 100);
</script>

<svelte:head
  ><title>Discover creators · MyFans</title><meta
    name="description"
    content="Search creators by name, interests, and category. Explore their work and communities."
  /></svelte:head
>
<main id="main-content">
  <div class="page-heading">
    <h1>Find your kind of people.</h1>
    <p>A searchable directory of creators and the work they love.</p>
  </div>
  <form
    role="search"
    aria-label="Creator directory"
    class="panel form-grid directory-search"
    method="GET"
    action="/creators"
  >
    <label
      >Search creators<input
        type="search"
        name="q"
        value={query}
        maxlength="100"
        placeholder="Name, handle, or interests"
      /></label
    >
    <label
      >Creator category<select name="category" value={category}
        ><option value="">All categories</option
        >{#each data.directory?.categories ?? [] as item (item)}<option value={item}>{item}</option
          >{/each}</select
      ></label
    >
    <button class="button-primary" disabled={!!navigating.to}
      ><Search size={16} />Search creators</button
    >
    {#if filtered}<a class="text-button" href="/creators">Clear filters</a>{/if}
  </form>
  {#if error || data.error}<section class="panel empty-panel">
      <Compass size={30} />
      <h2>Let’s try that again.</h2>
      <p class="error-message" role="alert">{error || data.error}</p>
      <button class="button-secondary" onclick={retry} disabled={busy}
        ><RefreshCw size={16} />{busy ? 'Refreshing…' : 'Retry creator directory'}</button
      >
    </section>
  {:else if !data.directory?.creators.length}<section class="panel empty-panel">
      <Compass size={30} />
      <h2>{filtered ? 'No creators match yet.' : 'The next creator story starts here.'}</h2>
      <p>
        {filtered
          ? 'Try another name, interest, or category.'
          : 'Approved creator profiles will appear here.'}
      </p>
      {#if filtered}<a class="button-secondary" href="/creators">Explore all creators</a>{:else}<a
          class="button-primary"
          href="/studio">Become a creator <ArrowUpRight size={16} /></a
        >{/if}
    </section>
  {:else}<p class="directory-count" role="status">
      {data.directory.creators.length} creator{data.directory.creators.length === 1 ? '' : 's'} on this
      page{filtered ? ' matching your search' : ''}.
    </p>
    <div class="creator-grid">
      {#each data.directory.creators as creator (creator.id)}
        <article class="panel creator-card">
          {#if creator.cover_url}<img
              class="creator-cover"
              src={creator.cover_url}
              alt=""
              width="520"
              height="180"
              loading="lazy"
            />{:else}<div class="creator-cover cover-placeholder"><Compass size={28} /></div>{/if}
          <div class="creator-details">
            <div class="creator-identity">
              {#if creator.avatar_url}<img
                  src={creator.avatar_url}
                  alt=""
                  width="48"
                  height="48"
                  loading="lazy"
                />{:else}<span class="avatar-placeholder">{creator.display_name.charAt(0)}</span
                >{/if}
              <div>
                <h2>
                  <a href={`/?creator=${encodeURIComponent(creator.id)}#feed-panel`}
                    >{creator.display_name}</a
                  >
                </h2>
                <p>@{creator.handle}</p>
              </div>
            </div>
            <span class="status-pill">{creator.category}</span>
            <p class="creator-bio">{creator.bio}</p>
            <div class="creator-footer">
              <span
                >{creator.subscribed
                  ? 'Your subscription is active'
                  : `${money(creator.subscription_price_minor, creator.currency)}/month`}</span
              >{#if creator.subscribed}<Check size={16} />{/if}
            </div>
            <a
              class="button-secondary full-width"
              href={`/?creator=${encodeURIComponent(creator.id)}#feed-panel`}
              >Explore {creator.display_name} <ArrowUpRight size={15} /></a
            >
          </div>
        </article>
      {/each}
    </div>
  {/if}
  <CollectionPages
    nextCursor={data.directory?.next_cursor}
    label="Creator directory"
    nextLabel="More creators"
    firstLabel="Back to first creators"
  />
</main>

<style>
  .directory-search {
    display: flex;
    align-items: flex-end;
    gap: 16px;
    flex-wrap: wrap;
    margin-bottom: 28px;
  }
  .directory-search label {
    flex: 1;
    min-width: 170px;
  }
  .directory-search input,
  .directory-search select {
    width: 100%;
    min-width: 0;
    min-height: 44px;
  }
  .directory-search button {
    min-height: 44px;
  }
  .directory-count {
    font-size: 12px;
    color: var(--muted);
    margin: 24px 0 16px;
  }
  .creator-grid {
    display: grid;
    grid-template-columns: repeat(3, minmax(0, 1fr));
    gap: 22px;
  }
  .creator-card {
    padding: 0;
    overflow: hidden;
  }
  .creator-cover {
    width: 100%;
    height: 150px;
    object-fit: cover;
  }
  .cover-placeholder {
    display: flex;
    align-items: center;
    justify-content: center;
    background: #e7ecda;
  }
  .creator-details {
    padding: 22px;
  }
  .creator-identity {
    display: flex;
    gap: 12px;
    align-items: center;
    margin-bottom: 18px;
  }
  .creator-identity img,
  .avatar-placeholder {
    width: 48px;
    height: 48px;
    border-radius: 50%;
    object-fit: cover;
  }
  .avatar-placeholder {
    display: grid;
    place-items: center;
    background: #ece2d7;
  }
  .creator-identity h2 {
    font-size: 16px;
    line-height: 1.4;
  }
  .creator-identity h2 a:hover {
    text-decoration: underline;
  }
  .creator-identity p {
    font-size: 11px;
    color: var(--muted);
    margin-top: 3px;
  }
  .creator-bio {
    font-size: 12px;
    line-height: 1.9;
    white-space: pre-line;
    overflow-wrap: anywhere;
    margin: 14px 0;
  }
  .creator-footer {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    margin: 18px 0;
  }
  @media (max-width: 1250px) {
    .creator-grid {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
  @media (max-width: 760px) {
    .creator-grid {
      grid-template-columns: minmax(0, 1fr);
    }
    .directory-search {
      align-items: stretch;
    }
    .directory-search label {
      flex-basis: 100%;
    }
  }
</style>
