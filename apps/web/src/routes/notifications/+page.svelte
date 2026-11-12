<script lang="ts">
  import { page } from '$app/state';
  import NotificationsWorkspace from '#lib/components/NotificationsWorkspace.svelte';
  import type { PageProps } from './$types';
  let { data }: PageProps = $props();
  let boundary = $derived(`${data.user?.id ?? ''}:${page.url.searchParams.get('cursor') ?? ''}`);
</script>

{#key boundary}<NotificationsWorkspace {data} />{/key}
