export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        canvas: "rgb(var(--color-canvas) / <alpha-value>)",
        panel: "rgb(var(--color-panel) / <alpha-value>)",
        surface: "rgb(var(--color-surface) / <alpha-value>)",
        surfaceMuted: "rgb(var(--color-surface-muted) / <alpha-value>)",
        ink: "rgb(var(--color-ink) / <alpha-value>)",
        muted: "rgb(var(--color-muted) / <alpha-value>)",
        accent: "rgb(var(--color-accent) / <alpha-value>)",
        accentSoft: "rgb(var(--color-accent-soft) / <alpha-value>)",
        border: "rgb(var(--color-border) / <alpha-value>)",
        infoSoft: "rgb(var(--color-info-soft) / <alpha-value>)",
        infoInk: "rgb(var(--color-info-ink) / <alpha-value>)",
        warningSoft: "rgb(var(--color-warning-soft) / <alpha-value>)",
        warningInk: "rgb(var(--color-warning-ink) / <alpha-value>)",
        dangerSoft: "rgb(var(--color-danger-soft) / <alpha-value>)",
        dangerInk: "rgb(var(--color-danger-ink) / <alpha-value>)",
        neutralSoft: "rgb(var(--color-neutral-soft) / <alpha-value>)",
        neutralInk: "rgb(var(--color-neutral-ink) / <alpha-value>)",
      },
      fontFamily: {
        sans: ["IBM Plex Sans", "Segoe UI", "Tahoma", "sans-serif"],
        mono: ["JetBrains Mono", "Consolas", "monospace"],
      },
      boxShadow: {
        card: "var(--shadow-card)",
      },
    },
  },
  plugins: [],
};
