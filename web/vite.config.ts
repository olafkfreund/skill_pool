import { sveltekit } from '@sveltejs/kit/vite';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'vite';

export default defineConfig({
  plugins: [tailwindcss(), sveltekit()],
  server: {
    host: '127.0.0.1',
    port: 5173,
    strictPort: false,
    allowedHosts:
      process.env.SP_ALLOWED_HOSTS?.split(',')
        .map((s) => s.trim())
        .filter(Boolean) ?? ['localhost', '127.0.0.1', 'razer.lan', '.localhost', '.lan', '.local', '.nip.io'],
  },
});
