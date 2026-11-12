<script lang="ts">
  import { onMount } from 'svelte';
  import { FileText, Check } from '@lucide/svelte';
  let {
    mediaId,
    ready,
    oncomplete,
  }: { mediaId: string; ready: boolean; oncomplete: () => void | Promise<void> } = $props();
  let files = $state<FileList>();
  let busy = $state(false);
  let error = $state('');
  let message = $state('');
  let disposed = false;
  const controller = new AbortController();
  onMount(() => () => {
    disposed = true;
    controller.abort();
    files = undefined;
  });
  async function upload(event: SubmitEvent) {
    event.preventDefault();
    const file = files?.[0];
    if (!file || busy || disposed) return;
    const target = mediaId;
    busy = true;
    error = '';
    message = '';
    try {
      if (file.size > 8192) throw new Error('Choose a WebVTT file of 8 KB or less.');
      const vtt = await file.text();
      if (disposed || target !== mediaId) return;
      const response = await fetch(`/api/platform/media/${target}/captions`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ vtt }),
        signal: controller.signal,
      });
      const result = await response.json();
      if (disposed || target !== mediaId) return;
      if (!response.ok) throw new Error(result.message ?? 'Caption upload failed.');
      files = undefined;
      message = 'English captions saved.';
      await oncomplete();
    } catch (e) {
      if (!disposed && target === mediaId)
        error = e instanceof Error ? e.message : 'Caption upload failed.';
    } finally {
      if (!disposed) busy = false;
    }
  }
</script>

<div class="caption-upload">
  {#if ready}<p class="caption-ready"><Check size={14} />English captions ready</p>{:else}<p
      class="footnote"
    >
      English WebVTT captions are required before this video can be attached to a post.
    </p>{/if}
  <form onsubmit={upload}>
    <label for={`captions-${mediaId}`}
      ><FileText size={14} />{ready ? 'Replace' : 'Add'} English captions<input
        id={`captions-${mediaId}`}
        type="file"
        accept=".vtt,text/vtt"
        bind:files
        required
      /></label
    ><button class="button-secondary" disabled={busy || !files?.length}
      >{busy ? 'Saving…' : 'Upload captions'}</button
    >
  </form>
  <p class="footnote">
    Plain WebVTT, up to 8 KB. Include timestamps and spoken dialogue; markup, STYLE, and REGION
    blocks are not supported.
  </p>
  {#if error}<p class="error-message" role="alert">{error}</p>{/if}{#if message}<p
      class="status-message"
      role="status"
    >
      {message}
    </p>{/if}
</div>

<style>
  .caption-upload {
    padding: 0 0 18px;
    display: grid;
    gap: 12px;
  }
  .caption-ready {
    display: flex;
    align-items: center;
    gap: 7px;
    font-size: 11px;
    color: #576f43;
  }
  .caption-upload form {
    display: flex;
    align-items: end;
    gap: 10px;
    flex-wrap: wrap;
  }
  .caption-upload label {
    display: grid;
    gap: 7px;
    font-size: 11px;
  }
  .caption-upload label :global(svg) {
    display: none;
  }
  .caption-upload input {
    width: 100%;
    max-width: 230px;
    font-size: 11px;
  }
  .caption-upload .button-secondary {
    font-size: 11px;
    padding: 10px 12px;
  }
  .caption-upload .error-message,
  .caption-upload .status-message {
    margin: 0;
  }
</style>
