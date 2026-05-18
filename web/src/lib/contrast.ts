/** Client-side WCAG contrast helpers — mirrors what the server enforces on save. */

function parseHex(hex: string): [number, number, number] | null {
  const s = hex.startsWith('#') ? hex.slice(1) : hex;
  if (s.length === 3) {
    const [r, g, b] = s.split('').map((c) => parseInt(c + c, 16));
    if ([r, g, b].some(Number.isNaN)) return null;
    return [r, g, b];
  }
  if (s.length === 6 || s.length === 8) {
    const r = parseInt(s.slice(0, 2), 16);
    const g = parseInt(s.slice(2, 4), 16);
    const b = parseInt(s.slice(4, 6), 16);
    if ([r, g, b].some(Number.isNaN)) return null;
    return [r, g, b];
  }
  return null;
}

function relLuminance(hex: string): number | null {
  const rgb = parseHex(hex);
  if (!rgb) return null;
  const channel = (c: number) => {
    const s = c / 255;
    return s <= 0.03928 ? s / 12.92 : Math.pow((s + 0.055) / 1.055, 2.4);
  };
  const [r, g, b] = rgb;
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b);
}

export function contrastRatio(a: string, b: string): number | null {
  const la = relLuminance(a);
  const lb = relLuminance(b);
  if (la === null || lb === null) return null;
  const [lighter, darker] = la > lb ? [la, lb] : [lb, la];
  return (lighter + 0.05) / (darker + 0.05);
}

export function wcagBadge(ratio: number | null): {
  level: 'AAA' | 'AA' | 'AA-large' | 'fail';
  label: string;
} {
  if (ratio === null) return { level: 'fail', label: 'invalid' };
  if (ratio >= 7) return { level: 'AAA', label: `${ratio.toFixed(2)}:1 — AAA` };
  if (ratio >= 4.5) return { level: 'AA', label: `${ratio.toFixed(2)}:1 — AA` };
  if (ratio >= 3) return { level: 'AA-large', label: `${ratio.toFixed(2)}:1 — large text only` };
  return { level: 'fail', label: `${ratio.toFixed(2)}:1 — fails WCAG` };
}
