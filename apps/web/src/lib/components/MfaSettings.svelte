<script lang="ts">
  import { onMount } from 'svelte';
  import { refreshAll } from '$app/navigation';
  import { ShieldCheck, Download } from '@lucide/svelte';
  import Dialog from './Dialog.svelte';
  let {
    user,
    remaining = 0,
  }: {
    user: { id: string; mfa_enabled: boolean; mfa_recovery_required: boolean };
    remaining?: number;
  } = $props();
  let secret = $state('');
  let uri = $state('');
  let code = $state('');
  let currentPassword = $state('');
  let rotating = $state(false);
  let recoveryCodes = $state.raw<string[]>([]);
  let saved = $state(false);
  let busy = $state(false);
  let error = $state('');
  let message = $state('');
  let disposed = false;
  let controller: AbortController | undefined;
  onMount(() => () => {
    disposed = true;
    controller?.abort();
    secret = '';
    uri = '';
    code = '';
    currentPassword = '';
    recoveryCodes = [];
  });
  async function action(body: Record<string, string>) {
    if (busy) return;
    busy = true;
    error = '';
    message = '';
    const current = new AbortController();
    controller = current;
    try {
      const response = await fetch('/api/platform/account/mfa', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
        signal: current.signal,
      });
      const result = await response.json();
      if (disposed || current.signal.aborted) return;
      if (!response.ok)
        throw new Error(result.message ?? 'Authenticator settings could not be saved.');
      if (body.action === 'setup') {
        secret = result.secret;
        uri = result.otpauth_url ?? result.uri ?? result.otpauth_uri ?? '';
      } else {
        secret = '';
        uri = '';
        code = '';
        currentPassword = '';
        rotating = false;
        if (Array.isArray(result.recovery_codes) && result.recovery_codes.length) {
          recoveryCodes = result.recovery_codes;
          saved = false;
        }
        message = result.message ?? 'Authenticator verification refreshed.';
        await refreshAll();
      }
    } catch (e) {
      if (!disposed && !current.signal.aborted)
        error = e instanceof Error ? e.message : 'Authenticator settings could not be saved.';
    } finally {
      if (!disposed) {
        busy = false;
        if (body.action !== 'setup') code = '';
      }
    }
  }
  function enable(event: SubmitEvent) {
    event.preventDefault();
    void action({ action: 'enable', code });
  }
  function rotate(event: SubmitEvent) {
    event.preventDefault();
    void action({ action: 'recovery_codes', code, current_password: currentPassword });
  }
  function closeRotation() {
    if (busy) return;
    rotating = false;
    currentPassword = '';
    code = '';
    error = '';
  }
  function closeCodes() {
    message = saved
      ? 'Recovery codes saved. Keep them separate from your authenticator.'
      : 'Recovery codes dismissed. Generate a new set if you did not save them.';
    recoveryCodes = [];
    saved = false;
    secret = '';
    uri = '';
    code = '';
    currentPassword = '';
  }
  function downloadCodes() {
    if (!recoveryCodes.length) return;
    const text = `MyFans recovery codes\nEach code can be used only once. Keep this file private.\n\n${recoveryCodes.join('\n')}\n`;
    const url = URL.createObjectURL(new Blob([text], { type: 'text/plain;charset=utf-8' }));
    const link = document.createElement('a');
    link.href = url;
    link.download = `myfans-recovery-codes-${new Date().toISOString().slice(0, 10)}.txt`;
    document.body.append(link);
    link.click();
    link.remove();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
</script>

<section class="panel" aria-labelledby="mfa-heading" aria-busy={busy}>
  <h2 id="mfa-heading"><ShieldCheck size={18} /> Authenticator verification</h2>
  <p class="section-description">
    Add a second step when signing in. Recent authenticator verification is required for hosted
    creator onboarding, creator review requests, payouts, and administrative review actions.
  </p>
  {#if user.mfa_recovery_required}<div class="recovery-notice" role="status">
      <strong>Replace your authenticator.</strong>
      <p>
        You signed in with a recovery code. Authenticator verification remains enabled, and
        privileged actions require a replacement authenticator. Set it up below to restore
        verification and receive fresh recovery codes.
      </p>
    </div>{:else if user.mfa_enabled}<p class="status-message">
      Authenticator verification is enabled.
    </p>{/if}
  {#if error && !rotating}<p class="error-message" role="alert">{error}</p>{/if}
  {#if message}<p class="status-message" role="status">{message}</p>{/if}
  {#if secret}<form class="form-grid" onsubmit={enable}>
      <p>Add this secret to your authenticator app, then enter its current six-digit code.</p>
      <code class="mfa-secret">{secret}</code>
      {#if uri}<a class="text-button" href={uri}>Open authenticator setup</a>{/if}
      <label
        >Authenticator code<input
          bind:value={code}
          inputmode="numeric"
          autocomplete="one-time-code"
          pattern={'[0-9]{6}'}
          maxlength="6"
          required
        /></label
      >
      <div class="form-actions">
        <button class="button-primary" disabled={busy}>Verify and enable</button>
        <button
          class="button-secondary"
          type="button"
          disabled={busy}
          onclick={() => {
            secret = '';
            uri = '';
            code = '';
            error = '';
          }}>Cancel setup</button
        >
      </div>
    </form>{:else if !user.mfa_enabled || user.mfa_recovery_required}<button
      class="button-secondary"
      disabled={busy}
      onclick={() => action({ action: 'setup' })}
      >{user.mfa_recovery_required
        ? 'Set up replacement authenticator'
        : 'Set up authenticator'}</button
    >
  {:else}<p class="section-description">
      {remaining} unused recovery {remaining === 1 ? 'code' : 'codes'} remaining. Each code works once
      and starts an authenticator recovery session.
    </p>
    <button
      class="button-secondary"
      disabled={busy}
      onclick={() => {
        rotating = true;
        code = '';
        currentPassword = '';
        error = '';
      }}>{remaining ? 'Rotate recovery codes' : 'Generate recovery codes'}</button
    >
    <details class="verification-refresh">
      <summary>Refresh verification for privileged actions</summary>
      <form class="form-grid" onsubmit={enable}>
        <label
          >Verification code<input
            bind:value={code}
            inputmode="numeric"
            autocomplete="one-time-code"
            pattern={'[0-9]{6}'}
            maxlength="6"
            required
          /></label
        ><button class="button-secondary" disabled={busy}>Refresh authenticator verification</button
        >
      </form>
    </details>
  {/if}
</section>
{#if rotating}<Dialog
    title={remaining ? 'Rotate recovery codes?' : 'Generate recovery codes?'}
    onclose={closeRotation}
  >
    <form class="form-grid" onsubmit={rotate}>
      <p>
        Your previous recovery codes and other signed-in sessions will be revoked. Save the new
        codes when they appear.
      </p>
      {#if error}<p class="error-message" role="alert">{error}</p>{/if}
      <label
        >Current password<input
          bind:value={currentPassword}
          type="password"
          autocomplete="current-password"
          maxlength="128"
          required
        /></label
      >
      <label
        >Authenticator code<input
          bind:value={code}
          inputmode="numeric"
          autocomplete="one-time-code"
          pattern={'[0-9]{6}'}
          maxlength="6"
          required
        /></label
      >
      <div class="form-actions">
        <button class="button-primary" disabled={busy}>Generate new recovery codes</button><button
          type="button"
          class="button-secondary"
          onclick={closeRotation}
          disabled={busy}>Cancel</button
        >
      </div>
    </form>
  </Dialog>{/if}
{#if recoveryCodes.length}<Dialog title="Save your recovery codes" onclose={closeCodes}>
    <div class="form-grid">
      <p>
        These ten codes appear only once. Each code can be used once if you lose access to your
        authenticator. Store them somewhere private and separate from your authenticator.
      </p>
      <ol class="recovery-codes">
        {#each recoveryCodes as recoveryCode (recoveryCode)}<li>
            <code>{recoveryCode}</code>
          </li>{/each}
      </ol>
      <button class="button-secondary" onclick={downloadCodes}
        ><Download size={16} /> Download recovery codes</button
      >
      <label class="saved-confirmation"
        ><input type="checkbox" bind:checked={saved} /><span
          >I have saved my recovery codes somewhere safe.</span
        ></label
      >
      <button class="button-primary" disabled={!saved} onclick={closeCodes}
        >Done, codes saved</button
      >
    </div>
  </Dialog>{/if}

<style>
  h2 {
    display: flex;
    align-items: center;
    gap: 9px;
  }
  .section-description {
    margin-bottom: 18px;
  }
  .recovery-notice {
    padding: 18px;
    border: 1px solid var(--border);
    background: var(--background);
    border-radius: 12px;
    margin-bottom: 20px;
  }
  .recovery-notice p {
    margin-top: 8px;
  }
  .mfa-secret {
    overflow-wrap: anywhere;
    font-size: 16px;
    padding: 14px;
    border-radius: 8px;
    background: var(--background);
    user-select: all;
  }
  .verification-refresh {
    margin-top: 20px;
  }
  .verification-refresh summary {
    cursor: pointer;
    font-size: 12px;
    margin-bottom: 14px;
  }
  .recovery-codes {
    padding-left: 20px;
  }
  .recovery-codes li {
    padding: 5px 0;
  }
  .recovery-codes code {
    overflow-wrap: anywhere;
    font-size: 11px;
    user-select: all;
  }
  .saved-confirmation {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    line-height: 1.6;
  }
  .saved-confirmation input {
    margin-top: 3px;
    flex-shrink: 0;
  }
</style>
