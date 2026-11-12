<script lang="ts">
  import { onMount } from 'svelte';
  import { goto, refreshAll } from '$app/navigation';
  import { Settings, LogOut, LockKeyhole } from '@lucide/svelte';
  import Dialog from '#lib/components/Dialog.svelte';
  import SafetySettings from '#lib/components/SafetySettings.svelte';
  import CollectionPages from '#lib/components/CollectionPages.svelte';
  import AccountMail from '#lib/components/AccountMail.svelte';
  import VerificationResend from '#lib/components/VerificationResend.svelte';
  import MfaSettings from '#lib/components/MfaSettings.svelte';
  import IntegrationSetup from '#lib/components/IntegrationSetup.svelte';
  import CreatorOnboarding from '#lib/components/CreatorOnboarding.svelte';
  import PrivacyErasure from '#lib/components/PrivacyErasure.svelte';
  import PaymentHistory from '#lib/components/PaymentHistory.svelte';
  import { page } from '$app/state';
  import type { PageProps } from './$types';
  import type { Subscription } from '#lib/route-data.ts';
  let { data }: PageProps = $props();
  let user = $derived(data.account?.user ?? data.user);
  let disposed = false;
  const controller = new AbortController();
  onMount(() => () => {
    disposed = true;
    controller.abort();
  });
  function currentAccount(owner: AccountDraft) {
    return !disposed && owner === draft;
  }
  let mailTransport = $derived(data.account?.mail_transport ?? 'sandbox');
  // Keep a user's unsaved input across unrelated load refreshes without mutating page data.
  let nameDraft = $state.raw<{ userId: string; value: string } | null>(null);
  let name = $derived(
    nameDraft && nameDraft.userId === user?.id ? nameDraft.value : (user?.display_name ?? ''),
  );
  class AccountDraft {
    currentPassword = $state('');
    password = $state('');
    busy = $state(false);
    error = $state('');
    message = $state('');
    confirmingCancel = $state<string | null>(null);
    confirmingDeactivation = $state(false);
    deactivationPassword = $state('');
    deactivationCode = $state('');
    deactivationError = $state('');
    readonly userId: string | undefined;
    constructor(userId: string | undefined) {
      this.userId = userId;
    }
  }
  let ownerId = $derived(user?.id);
  let draft = $derived(new AccountDraft(ownerId));
  const canCancel = (subscription: Subscription) =>
    ['active', 'past_due'].includes(subscription.status);
  function periodEnd(value: string | null) {
    if (!value) return null;
    const date = new Date(value);
    return Number.isFinite(date.getTime()) ? date.toLocaleDateString() : null;
  }
  async function exportAccount() {
    if (draft.busy) return;
    const owner = draft;
    draft.busy = true;
    draft.error = '';
    draft.message = '';
    try {
      const response = await fetch('/api/platform/account/export', { signal: controller.signal });
      const result = await response.json();
      if (!currentAccount(owner)) return;
      if (!response.ok) throw new Error(result.message ?? 'Unable to export your account.');
      const url = URL.createObjectURL(
        new Blob([JSON.stringify(result, null, 2)], { type: 'application/json' }),
      );
      const link = document.createElement('a');
      link.href = url;
      link.download = `myfans-account-${new Date().toISOString().slice(0, 10)}.json`;
      document.body.append(link);
      link.click();
      link.remove();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      draft.message =
        'Your account export has been downloaded. Media files are available separately in Creator Studio.';
    } catch (e) {
      if (currentAccount(owner))
        draft.error = e instanceof Error ? e.message : 'Unable to export your account.';
    } finally {
      if (currentAccount(owner)) owner.busy = false;
    }
  }
  async function deactivate(event: SubmitEvent) {
    event.preventDefault();
    if (draft.busy) return;
    const owner = draft;
    draft.busy = true;
    draft.deactivationError = '';
    try {
      await request('account/deactivate', {
        current_password: draft.deactivationPassword,
        ...(user?.mfa_enabled ? { mfa_code: draft.deactivationCode } : {}),
      });
      if (!currentAccount(owner)) return;
      owner.deactivationPassword = '';
      owner.deactivationCode = '';
      owner.confirmingDeactivation = false;
      await goto('/auth?deactivated=1', { refreshAll: true });
    } catch (e) {
      if (currentAccount(owner))
        draft.deactivationError =
          e instanceof Error ? e.message : 'Unable to deactivate your account.';
    } finally {
      if (currentAccount(owner)) owner.busy = false;
    }
  }
  async function request(path: string, body: unknown, method = 'POST') {
    const owner = draft;
    const response = await fetch(`/api/platform/${path}`, {
      method,
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(body),
      signal: controller.signal,
    });
    const result = await response.json();
    if (!currentAccount(owner)) throw new DOMException('The account view changed.', 'AbortError');
    if (!response.ok)
      throw new Error(result.message ?? result.error ?? 'Unable to save your changes.');
    return result;
  }
  async function act(task: (current: AccountDraft) => Promise<void>, refreshed?: () => void) {
    if (draft.busy) return;
    const owner = draft;
    draft.busy = true;
    draft.error = '';
    draft.message = '';
    try {
      await task(owner);
      if (!currentAccount(owner)) return;
      await refreshAll();
      if (currentAccount(owner)) refreshed?.();
    } catch (e) {
      if (currentAccount(owner))
        draft.error = e instanceof Error ? e.message : 'Unable to save your changes.';
    } finally {
      if (currentAccount(owner)) owner.busy = false;
    }
  }
  function saveProfile(event: SubmitEvent) {
    event.preventDefault();
    const submitted = nameDraft;
    void act(
      async (current) => {
        await request('account', { display_name: name }, 'PATCH');
        if (!currentAccount(current)) return;
        current.message = 'Your profile has been updated.';
      },
      () => {
        // A later edit belongs to the next save, even if this request finishes first.
        if (nameDraft === submitted) nameDraft = null;
      },
    );
  }
  function changePassword(event: SubmitEvent) {
    event.preventDefault();
    void act(async (current) => {
      await request('account/password', {
        current_password: current.currentPassword,
        password: current.password,
      });
      if (!currentAccount(current)) return;
      current.currentPassword = '';
      current.password = '';
      await goto('/auth', { refreshAll: true });
    });
  }
  function cancelSubscription(id: string) {
    void act(async (current) => {
      await request(`subscriptions/${id}`, {}, 'DELETE');
      if (!currentAccount(current)) return;
      current.confirmingCancel = null;
      current.message =
        'Subscription cancellation saved. Renewal stops at period end; past charges are not refunded. Existing access follows your paid period and payment status.';
    });
  }
  async function logout() {
    if (draft.busy) return;
    const owner = draft;
    draft.busy = true;
    draft.error = '';
    try {
      const response = await fetch('/api/auth/logout', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
        signal: controller.signal,
      });
      if (!currentAccount(owner)) return;
      if (!response.ok) throw new Error('Unable to sign out. Please try again.');
      await goto('/', { refreshAll: true });
    } catch (e) {
      if (currentAccount(owner))
        draft.error = e instanceof Error ? e.message : 'Unable to sign out.';
    } finally {
      if (currentAccount(owner)) owner.busy = false;
    }
  }
</script>

<svelte:head><title>Settings · MyFans</title></svelte:head>
<main id="main-content">
  <div class="page-heading">
    <h1>Make this space yours.</h1>
    <p>Your profile, security, and creator subscriptions.</p>
  </div>
  {#if page.url.searchParams.get('checkout') === 'returned'}<aside
      class="checkout-return-note"
      aria-label="Checkout return"
    >
      Returned from checkout. Check your payment status below; returning here does not confirm
      payment or grant access. If the result remains unclear, contact support with the checkout
      reference before trying again.
    </aside>{/if}
  {#if draft.error || data.error}<p class="error-message" role="alert">
      {draft.error || data.error}
    </p>{/if}{#if draft.message}<p class="status-message" role="status">{draft.message}</p>{/if}
  {#if !user}<section class="panel empty-panel">
      <Settings size={32} />
      <h2>A space of your own.</h2>
      <p>Sign in to manage your account.</p>
      <a class="button-primary" href="/auth">Sign in</a>
    </section>{:else}<div class="two-column">
      <div class="stack">
        <section class="panel">
          <h2>Your profile</h2>
          {#if user.role === 'admin'}<a class="text-button" href="/admin">Open administration</a
            >{/if}
          <p class="section-description">{user.email}</p>
          <div class="account-status">
            <span class="status-pill">{user.role}</span><span class="status-pill"
              >{user.email_verified ? 'Email verified' : 'Email verification pending'}</span
            >
          </div>
          {#if !user.email_verified}{#key `${user.id}:${mailTransport}`}<VerificationResend
                transport={mailTransport}
              />{/key}{/if}
          <form class="form-grid" onsubmit={saveProfile}>
            <label
              >Display name<input
                bind:value={
                  () => name,
                  (value) => {
                    if (user) nameDraft = { userId: user.id, value };
                  }
                }
                required
                minlength="2"
                maxlength="80"
                autocomplete="name"
              /></label
            ><button class="button-primary" disabled={draft.busy}>Save profile</button>
          </form>
        </section>
        <section class="panel">
          <h2>Your creator circle</h2>
          {#if !data.account?.subscriptions?.length}<p>You haven’t subscribed to a creator yet.</p>
            <a class="text-button" href="/creators">Discover creators</a
            >{/if}{#each data.account?.subscriptions ?? [] as subscription (subscription.id)}<div
              class="subscription-row"
            >
              <strong>{subscription.display_name}</strong>
              <p>
                {subscription.status}
                {#if periodEnd(subscription.current_period_end)}
                  · Current period ends {periodEnd(subscription.current_period_end)}
                {:else}
                  · Current period is not yet confirmed{/if}
              </p>
              {#if subscription.status === 'pending'}<p class="footnote">
                  Payment is pending; access is not confirmed. Contact support about an unresolved
                  payment before attempting another purchase.
                </p>{/if}
              {#if subscription.cancel_at_period_end && canCancel(subscription)}<span
                  class="status-pill">Cancels at period end</span
                >{:else if canCancel(subscription) && draft.confirmingCancel === subscription.id}<p
                  class="footnote"
                >
                  Cancel this subscription at the end of its current period?
                </p>
                <div class="form-actions">
                  <button
                    class="button-secondary"
                    onclick={() => cancelSubscription(subscription.id)}
                    disabled={draft.busy}>Confirm cancellation</button
                  ><button class="text-button" onclick={() => (draft.confirmingCancel = null)}
                    >Keep subscription</button
                  >
                </div>{:else if canCancel(subscription)}<button
                  class="text-button"
                  onclick={() => (draft.confirmingCancel = subscription.id)}
                  >Cancel at period end</button
                >{/if}
            </div>{/each}
          <CollectionPages
            nextCursor={data.account?.next_cursors?.subscriptions}
            parameter="subscriptions_cursor"
            label="Subscriptions"
            nextLabel="Older subscriptions"
          />
        </section>
        {#key user.id}<PaymentHistory
            payments={data.account?.payments}
            nextCursor={data.account?.next_cursors?.payments}
            failed={Boolean(data.error)}
          />{/key}
        {#if user.role === 'creator'}{#key user.id}<CreatorOnboarding
              userId={user.id}
              onboarding={data.account?.onboarding}
              identityMode={data.integrations?.identity}
              paymentMode={data.integrations?.payments}
            />{/key}{/if}
        {#key user.id}
          <IntegrationSetup
            status={data.integrations}
            initialError={data.integrationError}
            allowSetup={user.role === 'creator'}
          />
        {/key}
        <section class="panel">
          <h2>Your data and account</h2>
          <p class="section-description">
            Download your account profile, posts, comments, sent messages, and transaction records
            as JSON. Download approved media files separately from <a
              class="text-button"
              href="/studio">Creator Studio</a
            >.
          </p>
          <button class="button-secondary" onclick={exportAccount} disabled={draft.busy}
            >Export account data</button
          >
          <div class="deactivation-section">
            <h3>Deactivate your account</h3>
            <p class="section-description">
              Deactivation blocks sign-in, revokes all sessions, cancels subscription renewal flags,
              and hides creator posts. Account and transaction records are retained; this does not
              erase your data. Resolve any creator balance and payouts first.
            </p>
            {#if user.role === 'admin'}<p class="footnote">
                Administrator offboarding requires an operator-managed process.
              </p>{:else}<button
                class="button-secondary"
                disabled={draft.busy}
                onclick={() => {
                  draft.confirmingDeactivation = true;
                  draft.deactivationError = '';
                  draft.deactivationPassword = '';
                  draft.deactivationCode = '';
                }}>Deactivate account</button
              >{/if}
          </div>
        </section>
        {#key user.id}<SafetySettings />{/key}
        {#key user.id}<PrivacyErasure
            mfaEnabled={user.mfa_enabled}
            administrator={user.role === 'admin'}
          />{/key}
      </div>
      <div class="stack">
        {#key `${user.id}:${mailTransport}`}<AccountMail
            userId={user.id}
            transport={mailTransport}
            ontransport={(mode) => {
              mailTransport = mode;
            }}
          />{/key}
        <section class="panel">
          <h2><LockKeyhole size={18} /> Account security</h2>
          <form class="form-grid" onsubmit={changePassword}>
            <label
              >Current password<input
                type="password"
                bind:value={draft.currentPassword}
                autocomplete="current-password"
                required
              /></label
            ><label
              >New password<input
                type="password"
                bind:value={draft.password}
                autocomplete="new-password"
                minlength="12"
                maxlength="128"
                required
              /><small>Use at least 12 characters.</small></label
            ><button class="button-secondary" disabled={draft.busy}>Update password</button>
          </form>
        </section>
        {#key user.id}<MfaSettings
            {user}
            remaining={data.account?.recovery_codes_remaining ?? 0}
          />{/key}
        <section class="panel">
          <h2>Ready for a little break?</h2>
          <p class="section-description">
            Sign out of this device. Your saved posts and subscriptions stay with your account.
          </p>
          <button class="button-secondary" onclick={logout} disabled={draft.busy}
            ><LogOut size={16} />Sign out</button
          >
        </section>
      </div>
    </div>{/if}
  {#if draft.confirmingDeactivation && user}<Dialog
      title="Deactivate your account?"
      onclose={() => {
        if (!draft.busy) {
          draft.confirmingDeactivation = false;
          draft.deactivationPassword = '';
          draft.deactivationCode = '';
        }
      }}
    >
      <form class="form-grid" onsubmit={deactivate}>
        <p>
          Your account will be disabled and all sessions signed out. Creator posts will be hidden
          and subscription renewal flags cancelled. Records are retained under the platform
          retention policy; this action does not erase account data.
        </p>
        <p class="footnote">
          Export your account and download approved media before continuing. Accounts with unsettled
          creator balances cannot be deactivated.
        </p>
        {#if draft.deactivationError}<p class="error-message" role="alert">
            {draft.deactivationError}
          </p>{/if}
        <label
          >Confirm current password<input
            type="password"
            bind:value={draft.deactivationPassword}
            autocomplete="current-password"
            required
            maxlength="128"
          /></label
        >
        {#if user.mfa_enabled}<label
            >Deactivation authenticator code<input
              bind:value={draft.deactivationCode}
              inputmode="numeric"
              autocomplete="one-time-code"
              pattern={'[0-9]{6}'}
              maxlength="6"
              required
            /></label
          >{/if}
        <div class="form-actions">
          <button class="button-primary" disabled={draft.busy}>Confirm deactivation</button><button
            class="button-secondary"
            type="button"
            disabled={draft.busy}
            onclick={() => {
              draft.confirmingDeactivation = false;
              draft.deactivationPassword = '';
              draft.deactivationCode = '';
            }}>Keep account active</button
          >
        </div>
      </form>
    </Dialog>{/if}
</main>

<style>
  .checkout-return-note {
    padding: 18px;
    margin-bottom: 22px;
    border: 1px solid var(--border);
    border-radius: 12px;
    font-size: 13px;
    line-height: 1.8;
  }
  .section-description {
    margin-bottom: 20px;
  }
  .account-status {
    display: flex;
    gap: 10px;
    margin-bottom: 22px;
  }
  .subscription-row {
    padding: 20px 0;
    border-bottom: 1px solid var(--border);
  }
  .subscription-row:last-child {
    border: 0;
  }
  .subscription-row strong {
    font-size: 13px;
  }
  .subscription-row p {
    font-size: 11px;
    margin: 8px 0 12px;
  }
  .subscription-row > .text-button {
    display: inline-flex;
    align-items: center;
    min-height: 44px;
    scroll-margin-block: 110px;
  }
  .panel h2 {
    display: flex;
    align-items: center;
    gap: 9px;
  }
  .deactivation-section {
    margin-top: 24px;
    border-top: 1px solid var(--border);
    padding-top: 22px;
  }
  .deactivation-section h3 {
    font-size: 15px;
    margin-bottom: 10px;
  }
</style>
