<script lang="ts">
  import { page } from '$app/state';
  import MessagesWorkspace from '#lib/components/MessagesWorkspace.svelte';
  import type { PageProps } from './$types';
  let { data }: PageProps = $props();
  let requestedRecipient = $derived(page.url.searchParams.get('recipient'));
  let identityBoundary = $derived(
    `${data.user?.id ?? ''}:${requestedRecipient ?? ''}:${page.url.searchParams.get('cursor') ?? ''}`,
  );
</script>

{#key identityBoundary}<MessagesWorkspace {data} {requestedRecipient} />{/key}
