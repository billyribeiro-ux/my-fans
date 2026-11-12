<script lang="ts">
  import { page } from '$app/state';
  import { ArrowUpRight, Plus, Heart, Sparkles, Check } from '@lucide/svelte';
  import type { FeedCreator } from '#lib/contracts.ts';
  let {
    creators,
    onprofile,
    onsubscribe,
  }: {
    creators: FeedCreator[];
    onprofile: (creator: FeedCreator) => void;
    onsubscribe: (creator: FeedCreator) => void;
  } = $props();
  let suggested = $derived(creators.filter((creator) => !creator.subscribed).slice(0, 3));
  let subscribed = $derived(creators.filter((creator) => creator.subscribed).length);
  const money = (creator: FeedCreator) =>
    new Intl.NumberFormat('en-US', {
      style: 'currency',
      currency: creator.currency,
      maximumFractionDigits: 0,
    }).format(creator.subscription_price_minor / 100);
</script>

<aside class="rail">
  <div class="suggestions">
    <div class="rail-heading">
      <h2>Find your people</h2>
      <Sparkles size={16} />
    </div>
    <p class="rail-subtitle">A few creators we think you'll love.</p>
    <div class="suggested-list">
      {#each suggested as creator (creator.id)}<div class="suggestion">
          <button class="suggestion-profile" onclick={() => onprofile(creator)}
            >{#if creator.avatar_url}<img
                src={creator.avatar_url}
                alt=""
                width="44"
                height="44"
              />{:else}<span class="suggestion-avatar" aria-hidden="true"
                >{creator.display_name.charAt(0)}</span
              >{/if}<span
              ><strong>{creator.display_name}</strong><small
                >{creator.category} · {money(creator)}/mo</small
              ></span
            ></button
          ><button
            class="add-creator"
            onclick={() => onsubscribe(creator)}
            aria-label={`Subscribe to ${creator.display_name}`}><Plus size={17} /></button
          >
        </div>{/each}
    </div>
    {#if suggested.length === 0}<p class="all-followed">
        <Check size={15} /> You're in everyone's circle.
      </p>{/if}
    <a class="text-link" href="/creators">Explore all creators <ArrowUpRight size={15} /></a>
  </div>
  <div class="membership">
    <div class="membership-top">
      <span><Heart size={15} fill="currentColor" /></span><small>A LITTLE MORE CONNECTION</small>
    </div>
    <h2>Good things<br />happen here.</h2>
    <p>Go beyond the feed. Support the<br />people who inspire you.</p>
    <div class="membership-line">
      <span>YOUR CREATOR CIRCLE</span><span>{String(subscribed).padStart(2, '0')}</span>
    </div>
    <div class="member-avatars">
      {#each creators
        .filter((creator) => creator.subscribed)
        .slice(0, 4) as creator (creator.id)}{#if creator.avatar_url}<img
            src={creator.avatar_url}
            alt={creator.display_name}
            width="35"
            height="35"
          />{:else}<span class="member-initial" aria-label={creator.display_name}
            >{creator.display_name.charAt(0)}</span
          >{/if}{/each}<a href="/creators" aria-label="Discover more creators"><Plus size={16} /></a
      ><span>Make it yours.</span>
    </div>
  </div>
  <div class="values">
    <span class="values-icon"><Heart size={18} strokeWidth={1.4} /></span>
    <h3>Made for real connection.</h3>
    <p>Independent creators. Meaningful<br />communities. A space to be you.</p>
  </div>
  <footer>
    <span>{page.data.integrations?.payments === 'sandbox' ? 'MyFans sandbox' : 'MyFans'}</span><span
      >Creator-first, always.</span
    ><small>© {new Date().getUTCFullYear()} MyFans</small>
  </footer>
</aside>

<style>
  .rail {
    width: 282px;
    padding-top: 8px;
    position: sticky;
    top: 30px;
    align-self: start;
  }
  .suggestions {
    padding: 22px 20px;
    border: 1px solid var(--border);
    background: var(--surface);
    border-radius: 15px;
  }
  .rail-heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }
  .rail-heading h2 {
    font-size: 14px;
    letter-spacing: -0.25px;
  }
  .rail-heading :global(svg) {
    color: #89967d;
  }
  .rail-subtitle {
    color: var(--muted);
    font-size: 10px;
    margin-top: 8px;
  }
  .suggested-list {
    display: grid;
    gap: 21px;
    margin: 27px 0 23px;
  }
  .suggestion {
    display: flex;
    align-items: center;
    gap: 7px;
  }
  .suggestion-profile {
    display: flex;
    align-items: center;
    gap: 10px;
    text-align: left;
    min-width: 0;
  }
  .suggestion-profile img {
    width: 40px;
    height: 40px;
    border-radius: 50%;
    object-fit: cover;
  }
  .suggestion-avatar {
    display: grid;
    place-items: center;
    width: 40px;
    height: 40px;
    border-radius: 50%;
    background: #e7ecda;
    font-size: 17px;
    flex-shrink: 0;
  }
  .suggestion-profile strong {
    display: flex;
    gap: 4px;
    align-items: center;
    font-size: 11px;
  }
  .suggestion-profile small {
    display: block;
    color: var(--muted);
    font-size: 9px;
    margin-top: 5px;
  }
  .add-creator {
    margin-left: auto;
    width: 26px;
    height: 26px;
    display: grid;
    place-items: center;
    border: 1px solid var(--border);
    border-radius: 50%;
    color: #687560;
  }
  .add-creator:hover {
    background: #edf1e5;
  }
  .text-link {
    width: 100%;
    display: flex;
    justify-content: space-between;
    align-items: center;
    border-top: 1px solid var(--border);
    padding-top: 18px;
    font-size: 10px;
    font-weight: 600;
    color: #64745b;
  }
  .all-followed {
    display: flex;
    gap: 7px;
    font-size: 11px;
    margin: 24px 0;
    color: var(--muted);
  }
  .membership {
    margin-top: 23px;
    padding: 25px 23px 24px;
    background: #283a2b;
    color: #f6f4e9;
    border-radius: 15px;
    position: relative;
    overflow: hidden;
  }
  .membership::after {
    content: '';
    width: 170px;
    height: 170px;
    border: 1px solid #ffffff0b;
    border-radius: 50%;
    position: absolute;
    top: -55px;
    right: -65px;
    pointer-events: none;
  }
  .membership-top {
    display: flex;
    gap: 9px;
    align-items: center;
    color: #bdc9ae;
  }
  .membership-top span {
    width: 25px;
    height: 25px;
    display: grid;
    place-items: center;
    border: 1px solid #ffffff25;
    border-radius: 50%;
  }
  .membership-top small {
    font-size: 7px;
    letter-spacing: 1.4px;
  }
  .membership h2 {
    font-family: Georgia, serif;
    font-size: 34px;
    font-weight: 400;
    line-height: 1.1;
    letter-spacing: -0.6px;
    margin-top: 23px;
  }
  .membership p {
    font-size: 10px;
    color: #bfccb8;
    line-height: 1.8;
    margin-top: 13px;
  }
  .membership-line {
    margin-top: 27px;
    padding-top: 17px;
    border-top: 1px solid #ffffff1a;
    display: flex;
    justify-content: space-between;
    font-size: 8px;
    letter-spacing: 1.1px;
    color: #bec9b6;
  }
  .member-avatars {
    display: flex;
    align-items: center;
    margin-top: 15px;
    padding-left: 3px;
  }
  .member-avatars img {
    border-radius: 50%;
    border: 2px solid #283a2b;
    margin-left: -3px;
    object-fit: cover;
  }
  .member-initial {
    display: grid;
    place-items: center;
    width: 32px;
    height: 32px;
    border-radius: 50%;
    background: #8e9c7b;
    color: white;
    flex-shrink: 0;
  }
  .member-avatars a {
    width: 32px;
    height: 32px;
    border-radius: 50%;
    display: grid;
    place-items: center;
    border: 1px dashed #b1bca380;
    margin-left: 3px;
    color: #ced6c3;
  }
  .member-avatars span {
    font-size: 9px;
    margin-left: 12px;
    color: #c3cdbb;
  }
  .values {
    text-align: center;
    margin-top: 30px;
  }
  .values-icon {
    color: #84917a;
    display: block;
    margin-bottom: 11px;
  }
  .values h3 {
    font-size: 11px;
    font-weight: 500;
  }
  .values p {
    font-size: 10px;
    color: var(--muted);
    line-height: 1.9;
    margin-top: 9px;
  }
  footer {
    border-top: 1px solid var(--border);
    margin-top: 28px;
    padding: 18px 0;
    display: flex;
    gap: 14px;
    flex-wrap: wrap;
    color: #687360;
    font-size: 8px;
  }
  footer small {
    width: 100%;
    font-size: 8px;
  }
  @media (max-width: 1190px) {
    .rail {
      width: 250px;
    }
    .suggestions {
      padding: 20px 16px;
    }
    .membership {
      padding: 23px 19px;
    }
  }
  @media (max-width: 1030px) {
    .rail {
      display: none;
    }
  }
</style>
