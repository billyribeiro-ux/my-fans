<script lang="ts">
  import { page } from '$app/state';
  let {
    nextCursor,
    parameter = 'cursor',
    label,
    nextLabel = 'Older items',
    firstLabel = 'Back to newest',
    anchor = '',
  }: {
    nextCursor?: string | null;
    parameter?: string;
    label: string;
    nextLabel?: string;
    firstLabel?: string;
    anchor?: string;
  } = $props();
  let hasCursor = $derived(page.url.searchParams.has(parameter));
  function href(cursor?: string | null) {
    const url = new URL(page.url.href);
    if (cursor) url.searchParams.set(parameter, cursor);
    else url.searchParams.delete(parameter);
    url.hash = anchor;
    return `${url.pathname}${url.search}${url.hash}`;
  }
</script>

{#if nextCursor || hasCursor}<nav aria-label={`${label} pages`}>
    {#if hasCursor}<a class="button-secondary" href={href()}>{firstLabel}</a>{/if}{#if nextCursor}<a
        class="button-secondary"
        href={href(nextCursor)}
        rel="next">{nextLabel}</a
      >{/if}
  </nav>{/if}

<style>
  nav {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 12px;
    margin: 20px 0 8px;
  }
</style>
