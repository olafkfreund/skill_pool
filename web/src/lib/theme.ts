export interface Theme {
  primary: string;
  primaryFg: string;
  accent: string;
  bg: string;
  fg: string;
  muted: string;
  mutedFg: string;
  border: string;
  radius: string;
  logoUrl?: string;
  brandName: string;
  /** When true the "Powered by skill-pool" footer credit is shown. Default true. */
  footerBranding: boolean;
  /**
   * One of the curated Google Fonts allowlist values (e.g. `"Inter"`,
   * `"system"`). When unset the UI falls back to the system stack.
   */
  fontFamily?: string;
}

export const DEFAULT_THEME: Theme = {
  primary: '#2563eb',
  primaryFg: '#ffffff',
  accent: '#0ea5e9',
  bg: '#ffffff',
  fg: '#0f172a',
  muted: '#f1f5f9',
  mutedFg: '#475569',
  border: '#e2e8f0',
  radius: '0.5rem',
  brandName: 'skill-pool',
  footerBranding: true,
};

/**
 * Resolve a picker value (the bare family name from the allowlist, or
 * `"system"`) into a full CSS `font-family` value with sensible fallbacks.
 * Wrap the chosen family in quotes so multi-word names ("IBM Plex Sans")
 * survive the trip through the CSS parser unscathed.
 */
function resolveFontStack(family: string | undefined): string {
  if (!family || family === 'system') {
    // System stack — same one Tailwind uses by default, kept in one place
    // so the admin picker default and the application default agree.
    return 'system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif';
  }
  // Monospace tail for code-oriented choices; serif/sans tail for the rest.
  // Heuristic is "name contains Mono"; small but covers JetBrains Mono today.
  const fallback = /mono/i.test(family)
    ? 'ui-monospace, SFMono-Regular, Menlo, monospace'
    : 'system-ui, sans-serif';
  return `"${family}", ${fallback}`;
}

/** Serialise a Theme to a CSS variables block suitable for inline injection. */
export function themeToCss(theme: Theme): string {
  return [
    `--sp-primary: ${theme.primary};`,
    `--sp-primary-fg: ${theme.primaryFg};`,
    `--sp-accent: ${theme.accent};`,
    `--sp-bg: ${theme.bg};`,
    `--sp-fg: ${theme.fg};`,
    `--sp-muted: ${theme.muted};`,
    `--sp-muted-fg: ${theme.mutedFg};`,
    `--sp-border: ${theme.border};`,
    `--sp-radius: ${theme.radius};`,
    `--sp-font-family: ${resolveFontStack(theme.fontFamily)};`,
  ].join(' ');
}
