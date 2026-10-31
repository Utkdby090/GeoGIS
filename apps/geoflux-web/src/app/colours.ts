// The app's colours as values, for drawing code that can't use CSS (charts, the 3D tower). Read
// from the stylesheet's custom properties, so there is one place they are defined, and read again
// when the system switches between light and dark.

import { useEffect, useState } from "react";

import type { Severity } from "../api/schemas";

export interface Tokens {
  text: string;
  muted: string;
  border: string;
  surface: string;
  sev: Record<Severity, string>;
}

/** The page's colours, read from its CSS so charts and the 3D tower follow light and dark mode. */
export function useColours(): Tokens {
  const read = (): Tokens => {
    const css = getComputedStyle(document.documentElement);
    const v = (name: string, fallback: string) => css.getPropertyValue(name).trim() || fallback;
    const muted = v("--muted", "#5d6673");
    return {
      text: v("--text", "#1b1f24"),
      muted,
      border: v("--border", "#d5dae1"),
      surface: v("--surface", "#ffffff"),
      sev: {
        none: muted,
        low: v("--sev-low", "#2f7d32"),
        medium: v("--sev-medium", "#b8860b"),
        high: v("--sev-high", "#d9480f"),
        critical: v("--sev-critical", "#b3261e"),
      },
    };
  };
  const [tokens, setTokens] = useState(read);
  useEffect(() => {
    const mq = window.matchMedia?.("(prefers-color-scheme: dark)");
    if (!mq) return;
    const changed = () => setTokens(read());
    mq.addEventListener("change", changed);
    return () => mq.removeEventListener("change", changed);
  }, []);
  return tokens;
}
