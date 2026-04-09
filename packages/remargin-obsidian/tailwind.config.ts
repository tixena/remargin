import type { Config } from "tailwindcss";

export default {
  content: ["./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        "bg-primary": "var(--background-primary)",
        "bg-secondary": "var(--background-secondary)",
        "bg-hover": "var(--background-modifier-hover)",
        "bg-border": "var(--background-modifier-border)",
        "text-normal": "var(--text-normal)",
        "text-muted": "var(--text-muted)",
        "text-faint": "var(--text-faint)",
        accent: "var(--interactive-accent)",
        "accent-hover": "var(--interactive-accent-hover)",
      },
      fontFamily: {
        mono: ["IBM Plex Mono", "ui-monospace", "monospace"],
        sans: ["Inter", "var(--font-interface)", "sans-serif"],
      },
    },
  },
  important: ".remargin-container",
} satisfies Config;
