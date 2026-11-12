<script lang="ts">
  import { onMount } from 'svelte';
  import LoadOlder from './LoadOlder.svelte';
  let {
    userId,
    transport = 'sandbox',
    ontransport,
  }: {
    userId: string;
    transport?: 'sandbox' | 'smtp';
    ontransport?: (mode: 'sandbox' | 'smtp') => void;
  } = $props();
  let deliveries = $state.raw<{ id: string; subject: string; body: string; created_at: string }[]>(
    [],
  );
  let open = $state(false);
  let busy = $state(false);
  let error = $state('');
  let cursor = $state<string | null>(null);
  let responseTransport = $state<'sandbox' | 'smtp' | null>(null);
  let effectiveTransport = $derived(
    transport === 'smtp' || responseTransport === 'smtp' ? 'smtp' : 'sandbox',
  );
  let disposed = false;
  let controller: AbortController | undefined;
  onMount(() => () => {
    disposed = true;
    controller?.abort();
    deliveries = [];
  });
  function mailLink(body: string) {
    const link = body.trim();
    return /^\/auth\?(verify|reset)=[A-Za-z0-9_-]+$/.test(link) ? link : null;
  }
  async function load(olderCursor?: string) {
    if (busy || effectiveTransport !== 'sandbox') return;
    busy = true;
    open = true;
    error = '';
    const identity = userId;
    const current = new AbortController();
    controller = current;
    try {
      const response = await fetch(
        `/api/platform/account/mail${olderCursor ? `?cursor=${encodeURIComponent(olderCursor)}` : ''}`,
        { signal: current.signal },
      );
      const result = await response.json();
      if (disposed || current.signal.aborted || identity !== userId) return;
      if (!response.ok) throw new Error(result.message ?? 'Unable to load sandbox inbox.');
      responseTransport = result.mode === 'smtp' ? 'smtp' : 'sandbox';
      ontransport?.(responseTransport);
      if (responseTransport === 'smtp') {
        deliveries = [];
        cursor = null;
        open = false;
        return;
      }
      deliveries = olderCursor
        ? [
            ...deliveries,
            ...result.deliveries.filter(
              (item: { id: string }) => !deliveries.some((old) => old.id === item.id),
            ),
          ]
        : result.deliveries;
      cursor = result.next_cursor ?? null;
    } catch (e) {
      if (!disposed && !current.signal.aborted)
        error = e instanceof Error ? e.message : 'Unable to load sandbox inbox.';
    } finally {
      if (!disposed) busy = false;
    }
  }
</script>

<section class="panel" aria-labelledby="mail-heading" aria-busy={busy}>
  <h2 id="mail-heading">
    {effectiveTransport === 'smtp' ? 'Email delivery' : 'Sandbox email inbox'}
  </h2>
  {#if effectiveTransport === 'smtp'}<p class="section-description">
      Check your email inbox. Verification and reset links are never available here.
    </p>
  {:else}<p class="section-description">
      This local inbox contains verification and recovery links for your account. Access to this
      inbox does not prove that you own the email address. Production requires real email delivery.
    </p>
    {#if error}<p class="error-message" role="alert">{error}</p>{/if}
    <button class="button-secondary" onclick={() => load()} disabled={busy}
      >{open ? 'Refresh inbox' : 'Open sandbox inbox'}</button
    >
    {#if open}{#each deliveries as item (item.id)}{@const link = mailLink(item.body)}
        <div class="subscription-row">
          <strong>{item.subject}</strong>
          <p>{new Date(item.created_at).toLocaleString()}</p>
          {#if link}<a class="text-button" href={link}>Open {item.subject.toLowerCase()} link</a
            >{:else}<p>{item.body}</p>{/if}
        </div>{/each}{#if !deliveries.length && !busy && !error}<p class="footnote inbox-empty">
          No messages in your sandbox inbox.
        </p>{/if}
      <LoadOlder {cursor} loading={busy} label="Older inbox messages" onload={load} />
    {/if}
  {/if}
</section>

<style>
  .section-description {
    margin-bottom: 20px;
  }
  .subscription-row {
    border-bottom: 1px solid var(--border);
    padding: 20px 0;
  }
  .subscription-row p {
    margin: 8px 0;
    font-size: 12px;
  }
  .subscription-row > .text-button {
    display: inline-flex;
    align-items: center;
    min-height: 44px;
    scroll-margin-block: 110px;
  }
  .inbox-empty {
    margin-top: 16px;
  }
</style>
