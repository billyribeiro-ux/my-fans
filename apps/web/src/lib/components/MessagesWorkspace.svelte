<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { SvelteMap } from 'svelte/reactivity';
  import { refreshAll } from '$app/navigation';
  import { MessageCircle, Send, Plus, RefreshCw } from '@lucide/svelte';
  import Dialog from '#lib/components/Dialog.svelte';
  import CollectionPages from '#lib/components/CollectionPages.svelte';
  import LoadOlder from '#lib/components/LoadOlder.svelte';
  import { startPolling } from '#lib/polling.ts';
  import { parseThread, type Conversation as LiveConversation } from '#lib/live-updates.ts';
  interface Conversation {
    id: string;
    recipient_id: string;
    display_name: string;
    last_message: string | null;
    updated_at: string;
    blocked: boolean;
    own_blocked: boolean;
  }
  interface Message {
    id: string;
    sender_id: string;
    body: string;
    created_at: string;
  }
  let {
    data,
    requestedRecipient = null,
  }: {
    requestedRecipient?: string | null;
    data: {
      user?: { id: string } | null;
      conversations: Conversation[];
      creators: { id: string; user_id: string; display_name: string }[];
      error?: string | null;
      next_cursor?: string | null;
    };
  } = $props();
  let selected = $state<string | null>(null);
  let messages = $state.raw<Message[]>([]);
  let recipient = $state('');
  let draft = $state('');
  const threadDrafts = new SvelteMap<string, string>();
  let busy = $state(false);
  let loading = $state(false);
  let error = $state('');
  let message = $state('');
  let nextMessageCursor = $state<string | null>(null);
  let confirmingBlock = $state<{ id: string; name: string; unblock: boolean } | null>(null);
  let generation = 0;
  let disposed = false;
  const mutations = new AbortController();
  let threadController: AbortController | undefined;
  let poller: ReturnType<typeof startPolling> | undefined;
  let liveConversation = $state.raw<LiveConversation | null>(null);
  let olderHistory = $state(false);
  let readingHistory = $state(false);
  let historyNode: HTMLDivElement | undefined;
  function attachHistory(node: HTMLDivElement) {
    historyNode = node;
    return () => {
      if (historyNode === node) historyNode = undefined;
    };
  }
  function historyScrolled(event: Event) {
    const node = event.currentTarget as HTMLDivElement;
    const reading = node.scrollHeight - node.scrollTop - node.clientHeight > 40;
    if (reading === readingHistory) return;
    readingHistory = reading;
    if (reading) poller?.pause();
    else poller?.start();
  }
  let syncError = $state('');
  let sessionDenied = $state(false);
  let accountRefreshStarted = false;
  function isolateSession() {
    sessionDenied = true;
    selected = null;
    messages = [];
    liveConversation = null;
    draft = '';
    threadDrafts.clear();
    threadController?.abort();
    poller?.stop();
    if (!accountRefreshStarted) {
      accountRefreshStarted = true;
      void refreshAll().catch(() => {});
    }
  }
  let conversation = $derived(
    liveConversation?.id === selected
      ? liveConversation
      : data.conversations?.find((item) => item.id === selected),
  );
  let conversations = $derived(
    (data.conversations ?? []).map((item) =>
      item.id === liveConversation?.id ? liveConversation : item,
    ),
  );
  function availabilityChanged() {
    poller?.resume();
  }
  async function pollThread(signal: AbortSignal) {
    if (!selected) return;
    const target = selected,
      version = generation,
      identity = data.user?.id;
    const result = parseThread(
      await api(`conversations/${target}/messages`, undefined, signal),
      target,
      identity ?? '',
    );
    if (
      signal.aborted ||
      target !== selected ||
      version !== generation ||
      identity !== data.user?.id
    )
      return;
    messages = result.messages;
    nextMessageCursor = result.next_cursor;
    liveConversation = result.conversation;
    syncError = '';
    await tick();
    if (
      !signal.aborted &&
      target === selected &&
      version === generation &&
      !readingHistory &&
      historyNode
    )
      historyNode.scrollTop = historyNode.scrollHeight;
  }
  async function toggleBlock(event: SubmitEvent) {
    event.preventDefault();
    if (disposed || !confirmingBlock || busy) return;
    const decision = confirmingBlock;
    poller?.pause();
    busy = true;
    error = '';
    message = '';
    try {
      const response = await fetch(`/api/platform/users/${decision.id}/block`, {
        method: decision.unblock ? 'DELETE' : 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
        signal: mutations.signal,
      });
      const result = await response.json();
      if (disposed) return;
      if (!response.ok) throw new Error(result.message ?? 'Block settings could not be saved.');
      confirmingBlock = null;
      draft = '';
      message = decision.unblock
        ? 'Your block has been removed. Other safety restrictions may still apply.'
        : 'User blocked. Existing paid access is unchanged; no financial changes were made.';
      await refreshAll();
      if (disposed) return;
      if (selected) await open(selected);
    } catch (e) {
      if (!disposed) error = e instanceof Error ? e.message : 'Block settings could not be saved.';
    } finally {
      if (!disposed) {
        busy = false;
        if (!sessionDenied) poller?.start();
      }
    }
  }
  async function api(path: string, body?: unknown, signal?: AbortSignal) {
    if (disposed) throw new Error('Request cancelled.');
    const response = await fetch(
      `/api/platform/${path}`,
      body === undefined
        ? { signal }
        : {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify(body),
            signal: signal ?? mutations.signal,
          },
    );
    const result = await response.json();
    if (disposed || signal?.aborted) throw new Error('Request cancelled.');
    if (!response.ok) {
      if (response.status === 401 || (response.status === 403 && body === undefined)) {
        selected = null;
        messages = [];
        liveConversation = null;
        draft = '';
        threadController?.abort();
        if (response.status === 401) isolateSession();
      }
      throw new Error(result.message ?? 'Unable to load your conversation.');
    }
    if (body === undefined && result.viewer_id !== data.user?.id) {
      isolateSession();
      throw new Error('Account identity changed. Refresh your account before continuing.');
    }
    return result;
  }
  async function open(id: string, cursor?: string) {
    if (disposed || (cursor && (loading || busy))) return;
    poller?.pause();
    threadController?.abort();
    const current = new AbortController();
    threadController = current;
    const requestGeneration = ++generation;
    const identity = data.user?.id;
    if (selected !== id) {
      if (selected) threadDrafts.set(selected, draft);
      draft = threadDrafts.get(id) ?? '';
    }
    selected = id;
    loading = true;
    error = '';
    if (!cursor) {
      messages = [];
      nextMessageCursor = null;
      olderHistory = false;
      readingHistory = false;
      liveConversation = null;
    }
    try {
      const result = parseThread(
        await api(
          `conversations/${id}/messages${cursor ? `?cursor=${encodeURIComponent(cursor)}` : ''}`,
          undefined,
          current.signal,
        ),
        id,
        identity ?? '',
      );
      if (
        !disposed &&
        selected === id &&
        requestGeneration === generation &&
        identity === data.user?.id
      ) {
        messages = cursor
          ? [
              ...result.messages.filter(
                (item: { id: string }) => !messages.some((old) => old.id === item.id),
              ),
              ...messages,
            ]
          : result.messages;
        nextMessageCursor = result.next_cursor ?? null;
        liveConversation = result.conversation;
        if (cursor) olderHistory = true;
        syncError = '';
        if (!cursor) {
          await tick();
          if (requestGeneration === generation && historyNode)
            historyNode.scrollTop = historyNode.scrollHeight;
        }
      }
    } catch (e) {
      if (!disposed && requestGeneration === generation && !current.signal.aborted)
        error = e instanceof Error ? e.message : 'Unable to load your conversation.';
    } finally {
      if (!disposed && requestGeneration === generation) {
        loading = false;
        if (!sessionDenied) poller?.start();
      }
    }
  }
  async function refreshThread() {
    if (disposed || !selected || busy || loading) return;
    const target = selected;
    try {
      await refreshAll();
      if (!disposed && selected === target) await open(target);
    } catch (e) {
      if (!disposed)
        error = e instanceof Error ? e.message : 'Unable to refresh your conversation.';
    }
  }
  async function start(event?: SubmitEvent) {
    event?.preventDefault();
    if (disposed || !recipient || busy) return;
    poller?.pause();
    busy = true;
    error = '';
    try {
      const result = await api('conversations', { recipient_id: recipient });
      await refreshAll();
      if (disposed) return;
      const id = result.conversation?.id ?? result.id;
      if (!id) throw new Error('The server did not return a conversation.');
      await open(id);
    } catch (e) {
      if (!disposed) error = e instanceof Error ? e.message : 'Unable to start conversation.';
    } finally {
      if (!disposed) {
        busy = false;
        if (!sessionDenied) poller?.start();
      }
    }
  }
  async function send(event: SubmitEvent) {
    event.preventDefault();
    if (disposed || !selected || !draft.trim() || busy || conversation?.blocked) return;
    const target = selected;
    poller?.pause();
    busy = true;
    error = '';
    try {
      await api(`conversations/${target}/messages`, { body: draft.trim() });
      draft = '';
      threadDrafts.delete(target);
      if (selected === target) await open(target);
      if (disposed) return;
      await refreshAll();
    } catch (e) {
      if (!disposed) error = e instanceof Error ? e.message : 'Unable to send message.';
    } finally {
      if (!disposed) {
        busy = false;
        if (!sessionDenied) poller?.start();
      }
    }
  }
  onMount(() => {
    poller = startPolling({
      enabled: () =>
        !!data.user?.id &&
        !!selected &&
        !sessionDenied &&
        !olderHistory &&
        !readingHistory &&
        !busy &&
        !loading,
      available: () => document.visibilityState === 'visible' && navigator.onLine,
      run: pollThread,
      onerror: () => {
        syncError = 'Automatic updates are temporarily unavailable. Retry or refresh messages.';
      },
      intervalMs: 10000,
    });
    poller.start();
    if (requestedRecipient && data.user?.id) {
      recipient = requestedRecipient;
      void start();
    } else if (data.user?.id && data.conversations?.[0]) void open(data.conversations[0].id);
    return () => {
      disposed = true;
      generation++;
      threadController?.abort();
      mutations.abort();
      poller?.stop();
      messages = [];
      liveConversation = null;
      draft = '';
      threadDrafts.clear();
    };
  });
</script>

<svelte:document onvisibilitychange={availabilityChanged} />
<svelte:window ononline={availabilityChanged} onoffline={availabilityChanged} />

<svelte:head><title>Messages · MyFans</title></svelte:head>
<main id="main-content">
  <div class="page-heading">
    <h1>A little more conversation.</h1>
    <p>Private messages with the creators in your circle.</p>
  </div>
  {#if error || data.error}<p class="error-message" role="alert">{error || data.error}</p>{/if}
  {#if message}<p class="status-message" role="status">{message}</p>{/if}
  {#if !data.user || sessionDenied}<section class="panel empty-panel">
      <MessageCircle size={32} />
      <h2>A space for conversation.</h2>
      <p>Sign in to send and receive private messages.</p>
      <a class="button-primary" href="/auth">Sign in</a>
    </section>{:else}<div class="message-layout">
      <aside class="panel conversation-list">
        <h2>Your conversations</h2>
        <form class="form-grid new-conversation" onsubmit={start}>
          <label
            >Message a creator<select bind:value={recipient} required
              ><option value="">Choose a creator</option
              >{#each (data.creators ?? []).filter((item) => item.user_id !== data.user?.id) as creator (creator.id)}<option
                  value={creator.user_id}>{creator.display_name}</option
                >{/each}</select
            ></label
          ><button class="button-secondary" disabled={busy || !recipient}
            ><Plus size={15} />Start conversation</button
          >
        </form>
        {#each conversations as item (item.id)}<button
            class={['conversation', { selected: selected === item.id }]}
            disabled={busy}
            onclick={() => open(item.id)}
            ><span class="conversation-avatar">{item.display_name?.charAt(0) ?? 'C'}</span><span
              ><strong>{item.display_name}</strong><small
                >{item.blocked ? 'Messaging restricted · ' : ''}{item.last_message ??
                  'Say hello.'}</small
              ></span
            ></button
          >{/each}{#if !data.conversations?.length}<p class="footnote">
            Start a conversation with a creator.
          </p>{/if}
        <CollectionPages
          nextCursor={data.next_cursor}
          label="Conversations"
          nextLabel="Older conversations"
        />
      </aside>
      <section class="panel thread" aria-label="Message thread">
        {#if selected}<div class="thread-heading">
            <h2>{conversation?.display_name ?? 'Your conversation'}</h2>
            {#if conversation}<button
                class="text-button"
                disabled={busy}
                onclick={() => {
                  if (conversation) {
                    confirmingBlock = {
                      id: conversation.recipient_id,
                      name: conversation.display_name,
                      unblock: conversation.own_blocked,
                    };
                    error = '';
                  }
                }}>{conversation.own_blocked ? 'Unblock user' : 'Block user'}</button
              >{/if}
            <button
              class="icon-button"
              onclick={refreshThread}
              disabled={loading || busy}
              aria-label="Refresh messages"><RefreshCw size={17} /></button
            >
          </div>
          <div
            class="message-history"
            {@attach attachHistory}
            onscroll={historyScrolled}
            aria-live="polite"
            aria-busy={loading}
          >
            <LoadOlder
              cursor={nextMessageCursor}
              loading={loading || busy}
              label="Older messages"
              onload={(cursor) => (selected ? open(selected, cursor) : undefined)}
            />
            {#if loading}<p class="footnote">
                Loading messages…
              </p>{:else}{#each messages as item (item.id)}<div
                  class={['message-bubble', { own: item.sender_id === data.user.id }]}
                >
                  <p>{item.body}</p>
                  <small>{new Date(item.created_at).toLocaleString()}</small>
                </div>{/each}{#if !messages.length}<p class="thread-empty">
                  Every good conversation starts with hello.
                </p>{/if}{/if}
          </div>
          {#if olderHistory}<p class="footnote" role="status">
              Automatic updates pause while you browse older history. Refresh messages to return to
              the newest page.
            </p>{:else if readingHistory}<p class="footnote" role="status">
              Automatic updates pause while you read earlier messages. Return to the bottom or
              refresh messages to resume.
            </p>{:else}<p class="footnote">
              Updates refresh every 10 seconds while this page is visible and online.
            </p>{/if}
          {#if syncError}<p class="error-message" role="status">
              {syncError}
              <button class="text-button" onclick={availabilityChanged}
                >Retry automatic updates</button
              >
            </p>{/if}
          {#if conversation?.blocked}<p class="restriction-note" role="status">
              New messages are unavailable while either person has blocked the other or the
              recipient’s account is disabled. Your conversation history remains readable.
            </p>{/if}
          <form class="message-compose" onsubmit={send}>
            <label class="sr-only" for="message-draft">Your message</label><textarea
              id="message-draft"
              bind:value={draft}
              required
              disabled={conversation?.blocked || busy}
              maxlength="4000"
              placeholder="Write a little hello…"></textarea><button
              class="button-primary"
              disabled={busy || !draft.trim() || conversation?.blocked}
              aria-label="Send message"><Send size={18} /></button
            >
          </form>{:else}<div class="thread-placeholder">
            <MessageCircle size={32} />
            <h2>Good conversations live here.</h2>
            <p>Select a conversation or say hello to a creator.</p>
          </div>{/if}
      </section>
    </div>{/if}
  {#if confirmingBlock}<Dialog
      title={confirmingBlock.unblock ? 'Unblock this person?' : 'Block this person?'}
      onclose={() => {
        if (!busy) confirmingBlock = null;
      }}
      ><form class="form-grid" onsubmit={toggleBlock}>
        <p><strong>{confirmingBlock.name}</strong></p>
        <p>
          {confirmingBlock.unblock
            ? 'Remove your block to allow this person’s posts and profile to appear again. Messaging remains restricted if they blocked you or their account is disabled.'
            : 'Hide this person’s posts and profile from discovery. Prevent new messages, likes, comments, and saves between you. Your message history remains readable.'}
        </p>
        <p class="footnote">
          Existing paid media access remains unchanged. Blocking and unblocking make no financial
          changes.
        </p>
        {#if error}<p class="error-message" role="alert">{error}</p>{/if}
        <div class="form-actions">
          <button class="button-primary" disabled={busy}
            >Confirm {confirmingBlock.unblock ? 'unblock' : 'block'}</button
          ><button
            class="button-secondary"
            type="button"
            disabled={busy}
            onclick={() => (confirmingBlock = null)}>Cancel</button
          >
        </div>
      </form></Dialog
    >{/if}
</main>

<style>
  .message-layout {
    display: grid;
    grid-template-columns: 290px 1fr;
    gap: 22px;
  }
  .new-conversation {
    margin: 22px 0;
  }
  .conversation {
    display: flex;
    gap: 12px;
    padding: 15px 10px;
    width: 100%;
    text-align: left;
    border-radius: 10px;
  }
  .conversation.selected {
    background: #edf2e5;
  }
  .conversation-avatar {
    height: 39px;
    width: 39px;
    display: grid;
    place-items: center;
    flex-shrink: 0;
    border-radius: 50%;
    background: #ece2d7;
  }
  .conversation strong {
    font-size: 12px;
  }
  .conversation small {
    display: block;
    margin-top: 7px;
    color: var(--muted);
    font-size: 11px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    max-width: 180px;
  }
  .thread {
    display: flex;
    flex-direction: column;
    min-height: 560px;
  }
  .thread-heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
    border-bottom: 1px solid var(--border);
    padding-bottom: 14px;
    gap: 12px;
    flex-wrap: wrap;
  }
  .thread-heading h2 {
    margin: 0;
  }
  .restriction-note {
    margin: 15px 0;
    font-size: 12px;
    line-height: 1.8;
    color: var(--muted);
  }
  .message-history {
    flex: 1;
    max-height: 550px;
    min-height: 330px;
    overflow: auto;
    padding: 20px 0;
    display: flex;
    flex-direction: column;
    gap: 15px;
  }
  .message-bubble {
    background: #f0f2e9;
    padding: 12px 16px;
    border-radius: 14px 14px 14px 3px;
    width: fit-content;
    max-width: 85%;
    overflow-wrap: anywhere;
  }
  .message-bubble.own {
    background: #dfe8d5;
    border-radius: 14px 14px 3px 14px;
    margin-left: auto;
  }
  .message-bubble p {
    font-size: 13px;
    white-space: pre-wrap;
    color: var(--ink);
  }
  .message-bubble small {
    font-size: 9px;
    color: var(--muted);
    display: block;
    margin-top: 7px;
  }
  .message-compose {
    display: flex;
    gap: 10px;
    align-items: end;
    border-top: 1px solid var(--border);
    padding-top: 20px;
  }
  .message-compose textarea {
    flex: 1;
    min-width: 0;
    height: 65px;
    max-height: 180px;
    resize: vertical;
    border: 1px solid var(--border);
    background: var(--background);
    padding: 12px;
    border-radius: 10px;
    font-size: 13px;
  }
  .message-compose button {
    height: 44px;
  }
  .thread-placeholder {
    margin: auto;
    text-align: center;
  }
  .thread-placeholder :global(svg) {
    color: #8a9b74;
    margin-bottom: 20px;
  }
  .thread-empty {
    font-size: 12px !important;
    text-align: center;
    margin: auto !important;
  }
  @media (max-width: 1100px) {
    .message-layout {
      grid-template-columns: 240px 1fr;
    }
  }
  @media (max-width: 850px) {
    .message-layout {
      grid-template-columns: 1fr;
    }
    .thread {
      min-height: 460px;
    }
    .conversation-list {
      max-height: 400px;
      overflow: auto;
    }
  }
</style>
