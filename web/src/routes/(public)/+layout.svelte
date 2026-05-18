<script lang="ts">
  let { children } = $props();
</script>

<main class="flex min-h-screen items-center justify-center px-4">
  {@render children?.()}
</main>
