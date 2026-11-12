<script lang="ts">
  import { refreshAll } from '$app/navigation';
  import { RefreshCw, Mail, ShieldCheck } from '@lucide/svelte';
  import type { PageProps } from './$types';
  let { data }: PageProps = $props();
  let busy = $state(false);
  let error = $state('');
  async function retry() {
    if (busy) return;
    busy = true;
    error = '';
    try {
      await refreshAll();
    } catch {
      error = 'Unable to refresh site information. Please try again.';
    } finally {
      busy = false;
    }
  }
</script>

<svelte:head
  ><title>Policies and operator · MyFans</title
  >{#if data.site?.mode !== 'production' || !data.site?.ready}<meta
      name="robots"
      content="noindex"
    />{/if}</svelte:head
>
<main id="main-content">
  <div class="page-heading">
    <h1>A space built with care.</h1>
    <p>Current policies, eligibility, and operator information for this installation.</p>
  </div>
  {#if !data.site}<section class="panel empty-panel">
      <ShieldCheck size={30} />
      <h2>Site information is unavailable.</h2>
      <p class="error-message" role="alert">
        {error || data.siteError || 'Please try again before registering.'}
      </p>
      <button class="button-secondary" onclick={retry} disabled={busy}
        ><RefreshCw size={16} />{busy ? 'Refreshing…' : 'Retry site information'}</button
      >
    </section>
  {:else}<div class="stack legal-content">
      {#if data.site.mode === 'sandbox'}<div class="sandbox-banner">
          <ShieldCheck size={20} />
          <p>
            Development sandbox policies. These documents describe this installation and are not
            production commercial terms.
          </p>
        </div>{/if}
      {#if !data.site.ready}<p class="error-message" role="alert">
          Current policies and operator details are not available yet. Registration is currently
          unavailable.
        </p>{/if}
      <section class="panel operator-information">
        <h2>Operator and contact</h2>
        <dl>
          <div>
            <dt>Operator</dt>
            <dd>{data.site.operator.legal_name ?? 'Not available'}</dd>
          </div>
          <div>
            <dt>Address</dt>
            <dd class="policy-body">{data.site.operator.address ?? 'Not available'}</dd>
          </div>
          <div>
            <dt>Support</dt>
            <dd>
              {#if data.site.operator.support_email}<a
                  class="text-button"
                  href={`mailto:${data.site.operator.support_email}`}
                  ><Mail size={15} />{data.site.operator.support_email}</a
                >{:else}Not available{/if}
            </dd>
          </div>
          <div>
            <dt>Privacy contact</dt>
            <dd>
              {#if data.site.operator.privacy_email}<a
                  class="text-button"
                  href={`mailto:${data.site.operator.privacy_email}`}
                  ><Mail size={15} />{data.site.operator.privacy_email}</a
                >{:else}Not available{/if}
            </dd>
          </div>
        </dl>
      </section>
      <section class="panel">
        <h2>Account eligibility</h2>
        <p>
          Registration requires confirmation that you are at least {data.site.eligibility
            .minimum_age} and live in an eligible country. This declaration does not constitute identity
          or age verification.
        </p>
        <p class="policy-meta">
          {data.site.eligibility.registration_enabled && data.site.ready
            ? 'Registration is available in the following eligible countries.'
            : 'Registration is currently unavailable.'}
        </p>
        {#if data.site.eligibility.countries.length}<ul>
            {#each data.site.eligibility.countries as country (country.code)}<li>
                {country.name}
              </li>{/each}
          </ul>{:else}<p>Eligible locations are not available yet.</p>{/if}
      </section>
      <nav class="policy-navigation" aria-label="Policy documents">
        {#each data.site.policies as policy (policy.kind)}<a
            class="button-secondary"
            href={`#${policy.kind}`}>{policy.title}</a
          >{/each}
      </nav>
      {#each data.site.policies as policy (policy.kind)}<section
          class="panel"
          id={policy.kind}
          aria-labelledby={`${policy.kind}-title`}
        >
          <h2 id={`${policy.kind}-title`}>{policy.title}</h2>
          <p class="policy-meta">Version {policy.version} · Updated {policy.updated_at}</p>
          {#each policy.sections as section (section)}<div class="policy-section">
              <h3>{section.heading}</h3>
              <p class="policy-body">{section.body}</p>
            </div>{/each}
        </section>{/each}
      {#if !data.site.policies.length}<section class="panel">
          <h2>Policy documents are unavailable.</h2>
          <p>The operator must supply this installation’s current policies before registration.</p>
        </section>{/if}
    </div>{/if}
</main>

<style>
  .legal-content {
    max-width: 900px;
  }
  .policy-navigation {
    display: flex;
    flex-wrap: wrap;
    gap: 12px;
  }
  .panel p,
  .panel li {
    font-size: 13px;
    line-height: 1.9;
  }
  .policy-body {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .policy-meta {
    margin-top: 12px;
    color: var(--muted);
  }
  .policy-section {
    margin-top: 26px;
  }
  .policy-section h3 {
    font-size: 16px;
    margin-bottom: 12px;
  }
  .operator-information dl {
    display: grid;
    gap: 18px;
    margin-top: 22px;
  }
  dt {
    color: var(--muted);
    font-size: 12px;
    margin-bottom: 6px;
  }
  dd {
    font-size: 13px;
  }
  dd a {
    overflow-wrap: anywhere;
  }
  ul {
    padding-left: 22px;
    margin-top: 12px;
  }
  section[id] {
    scroll-margin-top: 20px;
  }
</style>
