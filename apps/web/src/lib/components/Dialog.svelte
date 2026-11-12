<script lang="ts">
  import { X } from '@lucide/svelte';
  import type { Snippet } from 'svelte';

  let { title, onclose, children }: { title: string; onclose: () => void; children: Snippet } =
    $props();
  const id = $props.id();
  const titleId = `${id}-title`;
  function mountDialog(dialog: HTMLDialogElement) {
    dialog.showModal();
    return () => dialog.close();
  }
</script>

<dialog
  {@attach mountDialog}
  aria-labelledby={titleId}
  oncancel={(event) => {
    event.preventDefault();
    onclose();
  }}
>
  <div class="dialog-heading">
    <h2 id={titleId}>{title}</h2>
    <button class="icon-button" onclick={onclose} aria-label="Close dialog"><X size={20} /></button>
  </div>
  {@render children()}
</dialog>

<style>
  dialog {
    width: min(480px, calc(100vw - 32px));
    max-height: calc(100dvh - 48px);
    margin: auto;
    padding: 28px;
    border: 1px solid var(--border);
    border-radius: 24px;
    color: var(--ink);
    background: var(--surface);
    box-shadow: 0 24px 100px #1e211e30;
  }
  dialog::backdrop {
    background: #17211a55;
    backdrop-filter: blur(5px);
  }
  .dialog-heading {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 20px;
    margin-bottom: 22px;
  }
  h2 {
    font-size: 24px;
    letter-spacing: -0.8px;
  }
</style>
