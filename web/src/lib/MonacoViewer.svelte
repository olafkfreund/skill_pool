<script lang="ts">
  /**
   * Lazy Monaco viewer. Dynamic-imports the editor only when the component
   * mounts in the browser, so no Monaco code lands in the SSR bundle and the
   * initial HTML doesn't carry the editor weight.
   *
   * Worker config is deliberately omitted — Monaco falls back to main-thread
   * tokenisation, which is fine for SKILL.md-sized files. If we ever need
   * worker performance we wire `MonacoEnvironment.getWorker` here.
   */
  import { onMount, untrack } from 'svelte';

  type Props = {
    value: string;
    language?: string;
    readOnly?: boolean;
    height?: string;
    onChange?: (next: string) => void;
  };

  let {
    value,
    language = 'markdown',
    readOnly = false,
    height = '24rem',
    onChange,
  }: Props = $props();

  let container = $state<HTMLDivElement | null>(null);
  let editor: { dispose(): void; setValue(v: string): void; getValue(): string } | null = null;

  onMount(() => {
    if (!container) return;
    let disposed = false;

    (async () => {
      const monaco = await import('monaco-editor');
      if (disposed) return;

      const instance = monaco.editor.create(container!, {
        value: untrack(() => value),
        language,
        readOnly,
        minimap: { enabled: false },
        wordWrap: 'on',
        fontSize: 13,
        scrollBeyondLastLine: false,
        automaticLayout: true,
        theme: 'vs',
      });

      editor = {
        dispose: () => instance.dispose(),
        setValue: (v) => instance.setValue(v),
        getValue: () => instance.getValue(),
      };

      if (!readOnly && onChange) {
        instance.onDidChangeModelContent(() => onChange(instance.getValue()));
      }
    })();

    return () => {
      disposed = true;
      editor?.dispose();
      editor = null;
    };
  });
</script>

<div
  bind:this={container}
  style:height
  class="overflow-hidden rounded-[var(--sp-radius)] border border-[var(--sp-border)]"
></div>
