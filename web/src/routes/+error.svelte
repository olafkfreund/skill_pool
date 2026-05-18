<script lang="ts">
  import { page } from '$app/state';
</script>

<main class="mx-auto max-w-xl px-6 py-24 text-center">
  <p class="text-sm font-medium uppercase tracking-widest text-[var(--sp-muted-fg)]">
    {page.status}
  </p>
  <h1 class="mt-2 text-3xl font-semibold text-[var(--sp-fg)]">
    {page.error?.message ?? 'Something went wrong'}
  </h1>
  <a href="/" class="mt-8 inline-block text-[var(--sp-primary)] underline">← back home</a>
</main>
