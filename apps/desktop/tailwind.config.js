export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        canvas: "#f4f8f8",
        panel: "#ffffff",
        ink: "#0e1b1f",
        muted: "#5a6c70",
        accent: "#007f7a",
        accentSoft: "#e1f3f2",
        border: "#d7e3e2",
      },
      fontFamily: {
        sans: ["IBM Plex Sans", "Segoe UI", "Tahoma", "sans-serif"],
        mono: ["JetBrains Mono", "Consolas", "monospace"],
      },
      boxShadow: {
        card: "0 10px 24px rgba(14, 27, 31, 0.08)",
      },
    },
  },
  plugins: [],
};