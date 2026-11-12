<script lang="ts">
  let {
    cursor,
    loading = false,
    label,
    onload,
  }: {
    cursor?: string | null;
    loading?: boolean;
    label: string;
    onload: (cursor: string) => void | Promise<void>;
  } = $props();
</script>

{#if cursor}<div class="load-older">
    <button class="button-secondary" disabled={loading} onclick={() => cursor && onload(cursor)}
      >{loading ? 'Loading…' : label}</button
    >
  </div>{/if}

<style>
  .load-older {
    margin: 18px 0;
  }
</style>
