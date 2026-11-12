<script lang="ts">
  import { refreshAll } from '$app/navigation';
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { startPolling } from '#lib/polling.ts';
  import { parseNotifications, type Notification } from '#lib/live-updates.ts';
  import { Bell, Check } from '@lucide/svelte';
  import CollectionPages from '#lib/components/CollectionPages.svelte';
  let {
    data,
  }: {
    data: {
      user?: { id: string } | null;
      notifications: {
        id: string;
        title: string;
        body: string;
        read_at: string | null;
        created_at: string;
      }[];
      error?: string | null;
      next_cursor?: string | null;
    };
  } = $props();
  let error = $state('');
  let busyId = $state<string | null>(null);
  let live = $state.raw<{ notifications: Notification[]; next_cursor: string | null } | null>(null);
  let sessionDenied = $state(false);
  let notifications = $derived(sessionDenied ? [] : (live?.notifications ?? data.notifications));
  let nextCursor = $derived(live ? live.next_cursor : data.next_cursor);
  let syncError = $state('');
  let disposed = false;
  let poller: ReturnType<typeof startPolling> | undefined;
  const controller = new AbortController();
  let accountRefreshStarted = false;
  function isolateSession() {
    sessionDenied = true;
    live = null;
    poller?.stop();
    if (!accountRefreshStarted) {
      accountRefreshStarted = true;
      void refreshAll().catch(() => {});
    }
  }
  async function pollNotifications(signal: AbortSignal) {
    const identity = data.user?.id;
    const response = await fetch('/api/platform/notifications', { signal });
    const result = await response.json();
    if (disposed || signal.aborted || identity !== data.user?.id) return;
    if (!response.ok) {
      if (response.status === 401) isolateSession();
      throw new Error(result.message ?? 'Unable to refresh notifications.');
    }
    if (result.viewer_id !== identity) {
      isolateSession();
      throw new Error('Account identity changed. Refresh your account before continuing.');
    }
    live = parseNotifications(result, identity ?? '');
    syncError = '';
  }
  function availabilityChanged() {
    poller?.resume();
  }
  onMount(() => {
    poller = startPolling({
      enabled: () =>
        !!data.user?.id && !sessionDenied && !busyId && !page.url.searchParams.has('cursor'),
      available: () => document.visibilityState === 'visible' && navigator.onLine,
      run: pollNotifications,
      onerror: () => {
        if (!sessionDenied)
          syncError =
            'Automatic notification updates are temporarily unavailable. Retry or refresh this page.';
      },
      intervalMs: 15000,
    });
    poller.start();
    return () => {
      disposed = true;
      poller?.stop();
      controller.abort();
      live = null;
    };
  });
  async function markRead(id: string) {
    if (busyId || sessionDenied) return;
    poller?.pause();
    busyId = id;
    error = '';
    try {
      const response = await fetch(`/api/platform/notifications/${id}/read`, {
        method: 'PATCH',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
        signal: controller.signal,
      });
      const result = await response.json();
      if (disposed) return;
      if (!response.ok) {
        if (response.status === 401) isolateSession();
        throw new Error(result.message ?? 'Unable to update notification.');
      }
      live = null;
      await refreshAll();
    } catch (e) {
      if (!disposed) error = e instanceof Error ? e.message : 'Unable to update notification.';
    } finally {
      if (!disposed) {
        busyId = null;
        if (!sessionDenied) poller?.start();
      }
    }
  }
</script>

<svelte:document onvisibilitychange={availabilityChanged} />
<svelte:window ononline={availabilityChanged} onoffline={availabilityChanged} />

<svelte:head><title>Notifications · MyFans</title></svelte:head>
<main id="main-content">
  <div class="page-heading">
    <h1>Your little updates.</h1>
    <p>Messages, purchases, publishing, and the moments that matter.</p>
  </div>
  {#if error || data.error}<p class="error-message" role="alert">{error || data.error}</p>{/if}
  {#if data.user && !sessionDenied}<p class="footnote updates-note">
      {page.url.searchParams.has('cursor')
        ? 'Automatic updates pause while you browse older notifications. Return to the newest page to resume.'
        : 'Updates refresh every 15 seconds while this page is visible and online.'}
    </p>{/if}
  {#if syncError && !sessionDenied}<p class="error-message" role="status">
      {syncError}
      <button class="text-button" onclick={availabilityChanged}>Retry automatic updates</button>
    </p>{/if}
  <section class="panel">
    {#if !data.user || sessionDenied}<div class="empty-panel">
        <Bell size={32} />
        <h2>Keep your circle close.</h2>
        <p>Sign in to see your notifications.</p>
        <a class="button-primary" href="/auth">Sign in</a>
      </div>{:else if !notifications?.length}<div class="empty-panel">
        <Bell size={32} />
        <h2>All quiet in your corner.</h2>
        <p>New updates will appear here.</p>
      </div>{:else}{#each notifications as item (item.id)}<div
          class={['list-row', { unread: !item.read_at }]}
        >
          <div>
            <strong>{item.title}</strong>
            <p>{item.body}</p>
            <small>{new Date(item.created_at).toLocaleString()}</small>
          </div>
          {#if !item.read_at}<button
              class="button-secondary"
              onclick={() => markRead(item.id)}
              disabled={!!busyId}><Check size={15} />Mark read</button
            >{:else}<span class="status-pill">Read</span>{/if}
        </div>{/each}{/if}
    {#if data.user && !sessionDenied}<CollectionPages
        {nextCursor}
        label="Notifications"
        nextLabel="Older notifications"
      />{/if}
  </section>
</main>

<style>
  .updates-note {
    margin-bottom: 20px;
  }
  .list-row {
    align-items: start;
    padding: 22px 0;
  }
  .list-row p {
    margin-top: 9px;
  }
  .unread strong::before {
    content: '';
    display: inline-block;
    width: 6px;
    height: 6px;
    background: #cb8b9d;
    border-radius: 50%;
    margin-right: 10px;
  }
  @media (max-width: 760px) {
    .list-row {
      flex-wrap: wrap;
    }
  }
</style>
