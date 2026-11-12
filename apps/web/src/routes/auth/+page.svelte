<script lang="ts">
  import { page } from '$app/state';
  import AuthWorkspace from '#lib/components/AuthWorkspace.svelte';
  import type { PageProps } from './$types';
  let { data }: PageProps = $props();
  let formBoundary = $derived(`${data.user?.id ?? ''}:${page.url.search}`);
</script>

{#key formBoundary}<AuthWorkspace />{/key}
