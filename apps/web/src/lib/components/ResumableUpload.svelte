<script lang="ts">
  import { onMount } from 'svelte';
  import { Upload, Pause, X } from '@lucide/svelte';
  import {
    fileFingerprint,
    parseSavedUpload,
    parseUpload,
    validateUploadFile,
    type SavedUpload,
    type UploadSession,
  } from '#lib/resumable-upload.ts';
  let {
    userId,
    disabled = false,
    oncomplete,
  }: { userId: string; disabled?: boolean; oncomplete: () => Promise<void> | void } = $props();
  const id = $props.id();
  let files = $state<FileList>();
  let saved = $state.raw<SavedUpload | null>(null);
  let session = $state.raw<UploadSession | null>(null);
  let busy = $state(false);
  let checking = $state(true);
  let error = $state('');
  let message = $state('');
  let storageWarning = $state('');
  let denied = $state(false);
  let progress = $derived(session ? Math.floor((session.offset / session.size_bytes) * 100) : 0);
  let needsFile = $derived(!session || session.state === 'uploading');
  let expired = $derived(
    session ? Date.parse(session.expires_at) <= Date.now() && session.state === 'uploading' : false,
  );
  let controller: AbortController | undefined;
  let generation = 0;
  let disposed = false;
  const storageKey = () => `myfans:upload:${userId}`;
  function forget() {
    try {
      localStorage.removeItem(storageKey());
    } catch {
      /* Upload still works without local storage. */
    }
    saved = null;
    session = null;
    files = undefined;
  }
  function remember(value: SavedUpload) {
    saved = value;
    try {
      localStorage.setItem(storageKey(), JSON.stringify(value));
    } catch {
      storageWarning =
        'Reload recovery is unavailable in this browser. Keep this page open to finish.';
    }
  }
  function pause() {
    generation++;
    controller?.abort();
    controller = undefined;
    busy = false;
    checking = false;
    message = 'Upload paused. Reselect the same file after a reload to resume.';
  }
  async function api(path: string, signal: AbortSignal, init: RequestInit = {}) {
    const response = await fetch(`/api/platform/media/uploads${path}`, {
      ...init,
      signal,
      cache: 'no-store',
    });
    if (response.status === 401 || response.status === 403) {
      denied = true;
      forget();
      throw new Error('Sign in to the upload owner’s account to continue.');
    }
    const result = response.status === 204 ? null : await response.json();
    if (!response.ok) throw new Error(result?.message ?? 'Upload could not continue. Try again.');
    return result;
  }
  function current(token: number, owner: string, signal: AbortSignal) {
    return !disposed && !signal.aborted && generation === token && owner === userId;
  }
  onMount(() => {
    const owner = userId;
    const request = new AbortController();
    controller = request;
    const token = ++generation;
    async function restore() {
      try {
        let record: string | null = null;
        try {
          record = localStorage.getItem(storageKey());
        } catch {
          storageWarning =
            'Reload recovery is unavailable in this browser. Keep this page open to finish.';
        }
        if (!record) return;
        let value: SavedUpload;
        try {
          value = parseSavedUpload(JSON.parse(record));
        } catch {
          forget();
          return;
        }
        saved = value;
        const result = await api(`/${value.id}`, request.signal);
        if (!current(token, owner, request.signal)) return;
        session = parseUpload(result, value);
        message =
          session.state === 'uploading'
            ? 'Saved upload found. Choose the same file to continue from the verified server progress.'
            : session.state === 'cancelled'
              ? 'This saved upload was cancelled. Clear it to start another.'
              : 'The file was received. Finish to refresh its processing status.';
      } catch (e) {
        if (current(token, owner, request.signal))
          error = e instanceof Error ? e.message : 'Saved progress could not be loaded.';
      } finally {
        if (current(token, owner, request.signal)) {
          checking = false;
          controller = undefined;
        }
      }
    }
    void restore();
    return () => {
      disposed = true;
      generation++;
      controller?.abort();
      files = undefined;
    };
  });
  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy || checking || disabled || denied) return;
    const file = files?.[0];
    if (!saved && !file) return;
    const owner = userId;
    const token = ++generation;
    const request = new AbortController();
    controller = request;
    busy = true;
    error = '';
    message = '';
    try {
      let active: UploadSession;
      if (saved) {
        // Offset must be fetched again after any interruption or an ambiguous response.
        const result = await api(`/${saved.id}`, request.signal);
        if (!current(token, owner, request.signal)) return;
        active = parseUpload(result, saved);
      } else {
        validateUploadFile(file!);
        const fingerprint = await fileFingerprint(file!);
        if (!current(token, owner, request.signal)) return;
        const result = await api('', request.signal, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ size_bytes: file!.size, content_type: file!.type }),
        });
        if (!current(token, owner, request.signal)) return;
        active = parseUpload(result);
        if (active.size_bytes !== file!.size || active.content_type !== file!.type)
          throw new Error('Upload metadata did not match the selected file.');
        remember({
          id: active.id,
          size: file!.size,
          type: file!.type,
          lastModified: file!.lastModified,
          fingerprint,
        });
      }
      session = active;
      if (active.state === 'cancelled')
        throw new Error('This upload was cancelled. Clear it to start another.');
      if (active.state === 'uploading') {
        if (Date.parse(active.expires_at) <= Date.now())
          throw new Error('This upload expired. Cancel it and start another.');
        if (!file) throw new Error('Choose the same file to resume this upload.');
        validateUploadFile(file);
        const fingerprint = await fileFingerprint(file);
        if (!current(token, owner, request.signal)) return;
        if (
          !saved ||
          file.size !== saved.size ||
          file.type !== saved.type ||
          file.lastModified !== saved.lastModified ||
          fingerprint !== saved.fingerprint
        )
          throw new Error(
            'Choose the original file. Its name, size, type, or modification time does not match the saved upload.',
          );
        while (active.offset < active.size_bytes) {
          const start = active.offset;
          const end = Math.min(start + active.chunk_size, active.size_bytes);
          const result = await api(`/${active.id}`, request.signal, {
            method: 'PATCH',
            headers: { 'Content-Type': 'application/octet-stream', 'Upload-Offset': String(start) },
            body: file.slice(start, end),
          });
          if (!current(token, owner, request.signal)) return;
          const next = parseUpload(result, saved);
          if (next.offset !== end || next.state !== 'uploading')
            throw new Error('Server progress changed. Resume to verify the current offset.');
          active = next;
          session = next;
        }
      }
      const result = await api(`/${active.id}/complete`, request.signal, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
      });
      if (!current(token, owner, request.signal)) return;
      if (result?.media?.id !== active.id || typeof result.media.state !== 'string')
        throw new Error('Upload completion could not be verified. Retry to check its status.');
      forget();
      message = '';
      await oncomplete();
    } catch (e) {
      if (current(token, owner, request.signal))
        error = e instanceof Error ? e.message : 'Upload could not continue. Try again.';
    } finally {
      if (current(token, owner, request.signal)) {
        busy = false;
        controller = undefined;
      }
    }
  }
  async function cancel() {
    if (!saved || busy || checking) return;
    if (session?.state === 'cancelled') {
      forget();
      error = '';
      message = 'Saved upload cleared.';
      return;
    }
    const owner = userId;
    const token = ++generation;
    const request = new AbortController();
    controller = request;
    busy = true;
    error = '';
    try {
      await api(`/${saved.id}`, request.signal, {
        method: 'DELETE',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
      });
      if (!current(token, owner, request.signal)) return;
      forget();
      message = 'Upload cancelled.';
    } catch (e) {
      if (current(token, owner, request.signal))
        error = e instanceof Error ? e.message : 'Unable to cancel upload.';
    } finally {
      if (current(token, owner, request.signal)) {
        busy = false;
        controller = undefined;
      }
    }
  }
</script>

<form class="form-grid" onsubmit={submit}>
  <p class="section-description">
    JPEG, PNG, or WebP up to 10 MiB. MP4 or WebM up to 256 MiB and five minutes. Uploads can be
    paused and resumed.
  </p>
  {#if checking}<p role="status">Checking saved upload progress…</p>{/if}
  {#if needsFile}<div>
      <label
        >Choose media<input
          type="file"
          accept="image/jpeg,image/png,image/webp,video/mp4,video/webm"
          bind:files
          disabled={disabled || busy || checking || denied}
          required={!saved}
          aria-describedby={`${id}-upload-help`}
        /></label
      ><small id={`${id}-upload-help`}
        >After a reload, choose the original file again. File contents are never stored in your
        browser.</small
      >
    </div>{/if}
  {#if session}<div class="upload-progress">
      <label for={`${id}-media-progress`}>Uploaded {progress}%</label><progress
        id={`${id}-media-progress`}
        max={session.size_bytes}
        value={session.offset}>{progress}%</progress
      ><small>Upload expires {new Date(session.expires_at).toLocaleString()}.</small>
    </div>{/if}
  <div class="upload-actions">
    <button
      class="button-secondary"
      disabled={disabled ||
        busy ||
        checking ||
        denied ||
        expired ||
        session?.state === 'cancelled' ||
        (!saved && !files?.length)}
      ><Upload size={16} />{busy
        ? 'Uploading…'
        : saved
          ? needsFile
            ? 'Resume upload'
            : 'Finish upload'
          : 'Upload to private library'}</button
    >
    {#if busy}<button type="button" class="text-button" onclick={pause}
        ><Pause size={15} />Pause upload</button
      >{:else if saved}<button
        type="button"
        class="text-button"
        onclick={cancel}
        disabled={checking || session?.state === 'complete' || session?.state === 'assembling'}
        ><X size={15} />{session?.state === 'cancelled'
          ? 'Clear saved upload'
          : 'Cancel upload'}</button
      >{/if}
  </div>
  {#if message}<p class="status-message" role="status">{message}</p>{/if}
  {#if error}<p class="error-message" role="alert">{error}</p>{/if}
  {#if storageWarning}<p class="section-description">{storageWarning}</p>{/if}
  {#if denied}<a class="text-button" href="/auth">Sign in again</a>{/if}
</form>

<style>
  .upload-progress {
    display: grid;
    gap: 8px;
  }
  progress {
    width: 100%;
    accent-color: var(--olive);
  }
  .upload-progress label {
    font-size: 12px;
  }
  .upload-progress small {
    color: var(--muted);
    font-size: 11px;
  }
  .upload-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 14px;
  }
</style>
