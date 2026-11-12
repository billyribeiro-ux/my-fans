<script lang="ts">
  import '#lib/styles/app.css';
  import { page } from '$app/state';
  import { Bell, Heart } from '@lucide/svelte';
  import Sidebar from '#lib/components/Sidebar.svelte';
  import { onMount } from 'svelte';
  import type { LayoutProps } from './$types';
  let { children, data }: LayoutProps = $props();
  let hydrated = $state(false);
  onMount(() => {
    hydrated = true;
  });
  let active = $derived(
    page.url.pathname === '/'
      ? ['following', 'saved'].includes(page.url.searchParams.get('tab') ?? '')
        ? page.url.searchParams.get('tab')!
        : 'discover'
      : page.url.pathname.startsWith('/creators')
        ? 'discover'
        : page.url.pathname.split('/')[1],
  );
</script>

<div class="app-shell" data-hydrated={hydrated}>
  <a class="skip-link" href="#main-content">Skip to main content</a>
  <Sidebar
    {active}
    displayName={data.user?.display_name ?? 'Welcome, explorer'}
    avatarUrl={data.user?.avatar_url ?? ''}
    userRole={data.user?.role ?? 'guest'}
  />
  <div class="workspace">
    <header class="topbar">
      <a class="mobile-brand" href="/"><Heart size={21} fill="#df8ca3" strokeWidth={0} />myfans.</a>
      <span class="environment-badge"
        >{data.integrations?.payments === 'sandbox'
          ? 'SANDBOX · NO REAL CHARGES'
          : data.integrations?.payments === 'stripe'
            ? 'STRIPE CHECKOUT'
            : 'CHECKOUT UNAVAILABLE'}</span
      >
      <div class="topbar-tools">
        {#if data.user}<a class="icon-button" href="/notifications" aria-label="Notifications"
            ><Bell size={20} /></a
          >{:else}<a class="button-secondary" href="/auth">Sign in</a><a
            class="button-primary"
            href="/auth?mode=register">Join MyFans</a
          >{/if}
      </div>
    </header>
    {@render children()}
    <footer class="site-footer">
      <a href="/legal">Policies and operator</a><a href="/creators">Creator directory</a
      >{#if data.site?.operator.support_email}<a href={`mailto:${data.site.operator.support_email}`}
          >Contact support</a
        >{/if}<span
        >{data.site?.mode === 'sandbox'
          ? 'Development sandbox'
          : data.site?.mode === 'production'
            ? 'General creator content'
            : 'Site information unavailable'}</span
      >
    </footer>
  </div>
</div>

<style>
  .site-footer {
    display: flex;
    flex-wrap: wrap;
    gap: 18px;
    padding: 22px 0 12px;
    margin-top: 20px;
    font-size: 11px;
    color: var(--muted);
    border-top: 1px solid var(--border);
  }
  .site-footer a:hover {
    color: var(--ink);
    text-decoration: underline;
  }
</style>
