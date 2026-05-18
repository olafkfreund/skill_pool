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
}

export const DEFAULT_THEME: Theme = {
  primary: '#2563eb',
  primaryFg: '#ffffff',
  accent: '#0ea5e9',
  bg: '#ffffff',
  fg: '#0f172a',
  muted: '#f1f5f9',
  mutedFg: '#64748b',
  border: '#e2e8f0',
  radius: '0.5rem',
  brandName: 'skill-pool',
  footerBranding: true,
};

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
  ].join(' ');
}
