<script lang="ts">
  import { onMount } from 'svelte';
  import { page } from '$app/state';
  import { goto, refreshAll } from '$app/navigation';
  import { Heart, ArrowUpRight, ShieldCheck } from '@lucide/svelte';
  import { registrationAvailable, registrationConsent, type PublicSite } from '#lib/public-site.ts';
  let site = $derived(page.data.site as PublicSite | null);
  let mode = $derived(
    page.url.searchParams.has('verify')
      ? 'verify'
      : page.url.searchParams.has('reset')
        ? 'reset'
        : (page.url.searchParams.get('mode') ?? 'login'),
  );
  let deactivated = $derived(page.url.searchParams.get('deactivated') === '1');
  let name = $state('');
  let email = $state('');
  let password = $state('');
  let mfaCode = $state('');
  let recoveryCode = $state('');
  let verificationMethod = $state('authenticator');
  let token = $state('');
  let accepted = $state(false);
  let country = $state('');
  let registrationEnabled = $derived(registrationAvailable(site));
  let consentVersion = $state('');
  let policyVersion = $derived(
    JSON.stringify([
      country,
      site?.eligibility.minimum_age,
      site?.policies.map((policy) => [policy.kind, policy.version]),
    ]),
  );
  let currentConsent = $derived(accepted && consentVersion === policyVersion);
  let eligibleCountry = $derived(
    site?.eligibility.countries.some((item) => item.code === country) ?? false,
  );
  let busy = $state(false);
  let error = $state('');
  let message = $state('');
  let disposed = false;
  let controller: AbortController | undefined;
  onMount(() => () => {
    disposed = true;
    controller?.abort();
    password = '';
    mfaCode = '';
    recoveryCode = '';
    token = '';
  });
  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy) return;
    const current = new AbortController();
    controller = current;
    busy = true;
    error = '';
    message = '';
    try {
      const action =
        mode === 'verify'
          ? 'verify'
          : mode === 'register'
            ? 'register'
            : mode === 'recover'
              ? 'recover'
              : mode === 'reset'
                ? 'reset'
                : 'login';
      const body =
        action === 'recover'
          ? { email }
          : action === 'verify'
            ? { token: page.url.searchParams.get('verify') }
            : action === 'reset'
              ? {
                  token:
                    token ||
                    page.url.searchParams.get('reset') ||
                    page.url.searchParams.get('token'),
                  password,
                  ...verification(),
                }
              : action === 'register'
                ? {
                    email,
                    password,
                    display_name: name,
                    ...registrationConsent(site, country, currentConsent),
                  }
                : { email, password, ...verification() };
      const response = await fetch(`/api/auth/${action}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify(body),
        signal: current.signal,
      });
      const result = await response.json();
      if (disposed || current.signal.aborted) return;
      mfaCode = '';
      recoveryCode = '';
      if (!response.ok)
        throw new Error(result.message ?? result.error ?? 'Unable to complete this request.');
      if (action === 'verify') {
        message = 'Email verified. Your account is ready.';
        await refreshAll();
      } else if (action === 'recover') {
        message =
          result.message ?? 'If the address is registered, recovery instructions have been queued.';
      } else if (action === 'reset') {
        password = '';
        message = result.message ?? 'Password updated. You can sign in with your new password.';
      } else {
        password = '';
        await goto(result.user?.mfa_recovery_required ? '/settings' : '/', { refreshAll: true });
      }
    } catch (e) {
      if (!disposed && !current.signal.aborted)
        error = e instanceof Error ? e.message : 'Unable to complete this request.';
    } finally {
      if (!disposed) busy = false;
    }
  }
  function verification() {
    return verificationMethod === 'recovery'
      ? recoveryCode.trim()
        ? { recovery_code: recoveryCode.trim() }
        : {}
      : mfaCode
        ? { mfa_code: mfaCode }
        : {};
  }
  function changeMethod(event: Event) {
    verificationMethod = (event.currentTarget as HTMLSelectElement).value;
    mfaCode = '';
    recoveryCode = '';
    error = '';
  }
  function switchMode(next: string) {
    error = '';
    message = '';
    void goto(`/auth?mode=${next}`);
  }
  function confirmConsent(event: Event) {
    accepted = (event.currentTarget as HTMLInputElement).checked;
    consentVersion = accepted ? policyVersion : '';
  }
  async function refreshSite() {
    try {
      await refreshAll();
    } catch {
      error = 'Unable to refresh site information. Please try again.';
    }
  }
</script>

<svelte:head
  ><title
    >{mode === 'verify'
      ? 'Verify your email'
      : mode === 'register'
        ? 'Join'
        : mode === 'recover'
          ? 'Recover your account'
          : mode === 'reset'
            ? 'Reset your password'
            : 'Sign in'} · MyFans</title
  ></svelte:head
>
<main id="main-content" class="auth-layout">
  <section class="auth-story">
    <span class="auth-heart"><Heart size={32} fill="currentColor" /></span>
    <h1>Your people.<br />Your little corner<br />of the world.</h1>
    <p>A home for creators and the communities that make their work possible.</p>
    <div class="auth-note">
      <ShieldCheck size={20} /><span
        >{page.data.integrations?.payments === 'sandbox'
          ? 'Sandbox environment.'
          : 'A space for general creator content.'}<br />{page.data.integrations?.payments ===
        'sandbox'
          ? 'No real money changes hands.'
          : 'Review payment terms before checkout.'}</span
      >
    </div>
  </section>
  <section class="panel">
    <h2>
      {mode === 'verify'
        ? 'Confirm your email.'
        : mode === 'register'
          ? 'Make yourself at home.'
          : mode === 'recover'
            ? 'Find your way back.'
            : mode === 'reset'
              ? 'A fresh start.'
              : 'Good to see you again.'}
    </h2>
    <p class="auth-description">
      {mode === 'verify'
        ? 'Confirm this email verification link to activate publishing, purchases, and messaging.'
        : mode === 'register'
          ? 'Create your account and discover your circle.'
          : mode === 'recover'
            ? 'Enter the email address associated with your account.'
            : mode === 'reset'
              ? 'Use the recovery token from your email.'
              : 'Sign in to continue your story.'}
    </p>
    {#if error}<p class="error-message" role="alert">{error}</p>{/if}{#if message}<p
        class="status-message"
        role="status"
      >
        {message}
      </p>{/if}
    {#if deactivated}<p class="status-message" role="status">
        Account deactivated. All sessions have been revoked. Records remain subject to the platform
        retention policy; deactivation does not erase your data.
      </p>{/if}
    <form class="form-grid" onsubmit={submit}>
      {#if mode === 'register'}<label
          >Your name<input
            bind:value={name}
            autocomplete="name"
            minlength="2"
            maxlength="80"
            required
          /></label
        >{/if}{#if mode !== 'reset' && mode !== 'verify'}<label
          >Email address<input
            type="email"
            bind:value={email}
            autocomplete="email"
            maxlength="254"
            required
          /></label
        >{/if}{#if mode === 'reset' && !page.url.searchParams.get('reset') && !page.url.searchParams.get('token')}<label
          >Recovery token<input bind:value={token} autocomplete="off" required /></label
        >{/if}{#if mode !== 'recover' && mode !== 'verify'}<label
          >Password<input
            type="password"
            aria-label="Password"
            bind:value={password}
            autocomplete={mode === 'login' ? 'current-password' : 'new-password'}
            minlength={mode === 'login' ? 1 : 12}
            maxlength="128"
            required
          /><small>{mode === 'login' ? '' : 'Use at least 12 characters.'}</small></label
        >{/if}{#if mode === 'login' || mode === 'reset'}<details>
          <summary>Authenticator or recovery code, if enabled</summary>
          <label class="mfa-field"
            >Verification method<select
              aria-label="Verification method"
              value={verificationMethod}
              onchange={changeMethod}
            >
              <option value="authenticator">Authenticator app</option><option value="recovery"
                >Recovery code</option
              >
            </select></label
          >
          {#if verificationMethod === 'recovery'}<label class="mfa-field"
              >Recovery code<input
                type="text"
                bind:value={recoveryCode}
                autocomplete="off"
                autocapitalize="none"
                spellcheck={false}
                maxlength="64"
              /></label
            >
            <p class="recovery-help">
              Each recovery code works once. {mode === 'reset'
                ? 'After resetting your password, sign in with another unused recovery code and replace your authenticator in Settings.'
                : 'After sign-in, replace your authenticator in Settings to restore privileged verification.'}
            </p>
          {:else}<label class="mfa-field"
              >Six-digit code<input
                bind:value={mfaCode}
                inputmode="numeric"
                autocomplete="one-time-code"
                pattern={'[0-9]{6}'}
                maxlength="6"
              /></label
            >{/if}
        </details>{/if}{#if mode === 'register'}
        {#if !registrationEnabled}<p class="error-message" role="alert">
            {page.data.siteError ??
              'Registration is currently unavailable. Please try again later or contact support.'}
          </p>
          <button class="text-button" type="button" onclick={refreshSite}
            >Refresh site information</button
          >{/if}
        <label
          >Country of residence<select
            aria-label="Country of residence"
            bind:value={country}
            required
            disabled={!registrationEnabled}
            ><option value="">Choose an eligible country</option
            >{#each site?.eligibility.countries ?? [] as item (item.code)}<option value={item.code}
                >{item.name}</option
              >{/each}</select
          ><small>Your country must be included in the eligible locations.</small></label
        >
        <p class="recovery-help">
          General creator content only. Explicit adult content is not supported.
        </p>
        <label class="terms-check"
          ><input
            type="checkbox"
            checked={currentConsent}
            onchange={confirmConsent}
            required
          /><span
            >I confirm that I am at least {site?.eligibility.minimum_age ?? 18}, live in the
            selected country, agree to the
            <a href="/legal#terms" target="_blank" rel="noopener noreferrer">terms</a>
            and
            <a href="/legal#content" target="_blank" rel="noopener noreferrer">content policy</a>,
            and have read the
            <a href="/legal#privacy" target="_blank" rel="noopener noreferrer">privacy notice</a>
            and
            <a href="/legal#refunds" target="_blank" rel="noopener noreferrer">refund policy</a
            >.</span
          ></label
        >{/if}<button
        class="button-primary full-width"
        disabled={busy ||
          (mode === 'register' && (!currentConsent || !eligibleCountry || !registrationEnabled))}
        >{busy
          ? 'Please wait…'
          : mode === 'verify'
            ? 'Verify email'
            : mode === 'register'
              ? 'Create account'
              : mode === 'recover'
                ? 'Send recovery instructions'
                : mode === 'reset'
                  ? 'Reset password'
                  : 'Sign in'}<ArrowUpRight size={16} /></button
      >
    </form>
    <div class="auth-switch">
      {#if mode === 'login'}<button class="text-button" onclick={() => switchMode('recover')}
          >Forgot your password?</button
        >
        <p>
          New here? <button class="text-button" onclick={() => switchMode('register')}
            >Join MyFans</button
          >
        </p>{:else}<button class="text-button" onclick={() => switchMode('login')}
          >Back to sign in</button
        >{/if}
    </div>
  </section>
</main>

<style>
  .auth-layout {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 38px;
    max-width: 900px;
    margin: 28px auto;
  }
  .auth-story {
    border-radius: 22px;
    background: #e7ecda;
    padding: 40px 33px;
    display: flex;
    flex-direction: column;
    min-height: 470px;
  }
  .auth-heart {
    color: #c68597;
  }
  .auth-story h1 {
    font-family: Georgia, serif;
    font-size: 43px;
    font-weight: 400;
    line-height: 1.15;
    letter-spacing: -1px;
    margin: 28px 0 22px;
  }
  .auth-story p {
    font-size: 13px;
    line-height: 1.9;
    color: #596748;
  }
  .auth-note {
    display: flex;
    gap: 12px;
    align-items: center;
    margin-top: auto;
    padding-top: 35px;
    font-size: 11px;
    line-height: 1.7;
    color: #596748;
  }
  .auth-description {
    margin-bottom: 24px;
  }
  .auth-switch {
    margin-top: 24px;
    display: grid;
    gap: 18px;
    text-align: center;
    font-size: 12px;
  }
  .auth-switch p {
    font-size: 12px;
  }
  .form-grid .terms-check {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    font-weight: 400;
    line-height: 1.7;
  }
  .terms-check a {
    text-decoration: underline;
  }
  .terms-check input {
    margin-top: 4px;
  }
  summary {
    font-size: 12px;
    color: var(--muted);
    cursor: pointer;
  }
  .recovery-help {
    font-size: 12px;
    line-height: 1.7;
    margin-top: 12px;
  }
  .mfa-field {
    margin-top: 14px;
  }
  @media (max-width: 900px) {
    .auth-layout {
      gap: 20px;
    }
    .auth-story {
      padding: 28px;
    }
    .auth-story h1 {
      font-size: 35px;
    }
  }
  @media (max-width: 760px) {
    .auth-layout {
      grid-template-columns: 1fr;
      margin: 8px auto;
    }
    .auth-story {
      min-height: 0;
      padding: 28px;
    }
    .auth-story h1 {
      font-size: 30px;
      margin: 15px 0;
    }
    .auth-note {
      padding-top: 20px;
    }
  }
</style>
