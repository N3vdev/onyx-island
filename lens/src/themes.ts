// Pro visual pack: overlay colour presets and lasso stroke styles. Shared by
// the overlay (which applies them) and Settings (which lists them), so the two
// can never disagree about what exists. "classic" and "rainbow" are the free
// defaults and reproduce the app's original look exactly.

export interface ThemePreset {
  label: string;
  /** Pro-only presets are locked on the Free plan. */
  pro: boolean;
  /** Accent used when the user hasn't picked their own (still the default). */
  accent?: string;
  /** Overrides for the CSS custom properties declared in index.html :root. */
  vars: Record<string, string>;
}

export const DEFAULT_ACCENT = "#080808";

export const THEME_PRESETS: Record<string, ThemePreset> = {
  classic: { label: "Classic", pro: false, vars: {} },
  midnight: {
    label: "Midnight",
    pro: true,
    accent: "#8b7dff",
    vars: {
      "--ink": "#ececf4",
      "--paper": "#17171f",
      "--hover": "rgba(255, 255, 255, 0.1)",
      "--muted-on-dark": "#5b5b6b",
      "--muted-on-light": "#a3a3b3",
      "--region-border": "rgba(150, 140, 255, 0.42)",
      "--region-fill": "rgba(12, 10, 32, 0.28)",
      "--text-selection": "rgba(139, 125, 255, 0.62)",
      "--hint-border": "rgba(150, 140, 255, 0.4)",
    },
  },
  frost: {
    label: "Frost",
    pro: true,
    accent: "#1f6feb",
    vars: {
      "--ink": "#0f2340",
      "--paper": "#eef5ff",
      "--hover": "rgba(31, 111, 235, 0.12)",
      "--muted-on-dark": "#b9cbe6",
      "--muted-on-light": "#50698c",
      "--region-border": "rgba(170, 205, 255, 0.6)",
      "--region-fill": "rgba(210, 230, 255, 0.1)",
      "--text-selection": "rgba(31, 111, 235, 0.55)",
      "--hint-border": "rgba(160, 200, 255, 0.55)",
    },
  },
  ember: {
    label: "Ember",
    pro: true,
    accent: "#ff7a1a",
    vars: {
      "--ink": "#fff1e6",
      "--paper": "#1c1410",
      "--hover": "rgba(255, 170, 110, 0.12)",
      "--muted-on-dark": "#6e5446",
      "--muted-on-light": "#c9a58d",
      "--region-border": "rgba(255, 150, 80, 0.45)",
      "--region-fill": "rgba(30, 14, 4, 0.26)",
      "--text-selection": "rgba(255, 122, 26, 0.55)",
      "--hint-border": "rgba(255, 150, 80, 0.4)",
    },
  },
};

export interface LassoStyle {
  label: string;
  pro: boolean;
  /** Gradient stops traced around the selection. First and last should
   * match so the gradient wraps without a seam. */
  palette: readonly string[];
  glow: string;
}

export const LASSO_STYLES: Record<string, LassoStyle> = {
  rainbow: {
    label: "Rainbow",
    pro: false,
    palette: ["#4285f4", "#ea4335", "#fbbc05", "#34a853", "#4285f4"],
    glow: "rgba(90, 125, 245, 0.55)",
  },
  mono: {
    label: "Mono",
    pro: true,
    palette: ["#ffffff", "#cfcfd6", "#ffffff"],
    glow: "rgba(255, 255, 255, 0.45)",
  },
  neon: {
    label: "Neon",
    pro: true,
    palette: ["#00e5ff", "#ff2bd6", "#7c4dff", "#00e5ff"],
    glow: "rgba(0, 229, 255, 0.6)",
  },
  sunset: {
    label: "Sunset",
    pro: true,
    palette: ["#ff7a18", "#ff2e63", "#ffb800", "#ff7a18"],
    glow: "rgba(255, 110, 60, 0.55)",
  },
};

/** Falls back to the free default whenever the stored choice is unknown or
 * the plan doesn't include it, so a lapsed subscription quietly reverts the
 * look instead of breaking it. */
export function effectivePreset(name: string, paid: boolean): ThemePreset {
  const preset = THEME_PRESETS[name];
  return preset && (paid || !preset.pro) ? preset : THEME_PRESETS.classic;
}

export function effectiveLassoStyle(name: string, paid: boolean): LassoStyle {
  const style = LASSO_STYLES[name];
  return style && (paid || !style.pro) ? style : LASSO_STYLES.rainbow;
}

/** Readable text colour (near-black or near-white) on top of `hex`, using
 * WCAG relative luminance. */
export function relativeLuminance(hex: string): number {
  const rgb = hex.replace("#", "").match(/.{2}/g)?.map((v) => parseInt(v, 16) / 255) ?? [0, 0, 0];
  const [r, g, b] = rgb.map((v) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

export function contrastingInk(hex: string): string {
  return relativeLuminance(hex) > 0.179 ? "#111114" : "#fafafc";
}

/** The accent as used on the overlay's dark bars: an accent too dark to see
 * against them (the near-black default included) is shown as near-white. */
export function barAccent(hex: string): { fill: string; ink: string } {
  const fill = relativeLuminance(hex) < 0.06 ? "#f4f4f6" : hex;
  return { fill, ink: contrastingInk(fill) };
}
