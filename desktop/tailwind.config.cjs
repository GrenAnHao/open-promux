/** @type {import('tailwindcss').Config} */
module.exports = {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  darkMode: ["class"],
  theme: {
    container: {
      center: true,
      padding: "1rem",
    },
    extend: {
      colors: {
        // Terminal console palette - intentionally unrelated to cc-switch.
        // Background stack: deep carbon, slightly bluer than pure black.
        carbon: {
          950: "#080B10",
          900: "#0B0F14",
          800: "#11161D",
          700: "#151B23",
          600: "#1A2129",
          500: "#1F2731",
          400: "#2A3340",
          300: "#3A4452",
        },
        ink: {
          50: "#F4F8FA",
          100: "#E5ECF2",
          200: "#C6D0DC",
          300: "#9DA8B7",
          400: "#8B95A1",
          500: "#6E7785",
          600: "#5C6573",
          700: "#414957",
        },
        // Single accent (mint-cyan) used for primary call-to-action and
        // "system online" indicators. Avoid using it for decoration.
        mint: {
          200: "#A6F4DD",
          300: "#7CECCB",
          400: "#5BE7C4",
          500: "#48C5A8",
          600: "#319984",
        },
        amber: {
          400: "#FFB347",
          500: "#F09232",
        },
        coral: {
          400: "#FF6B6B",
          500: "#E04F4F",
        },
        sky: {
          300: "#7DD3FC",
        },
      },
      fontFamily: {
        // Latin glyphs come from a modern UI font, then we fall through to
        // the platform native CJK family. This mirrors the system-first
        // stack that cc-switch uses (without copying its primary blue).
        sans: [
          "Inter",
          "-apple-system",
          "BlinkMacSystemFont",
          '"Segoe UI"',
          "Roboto",
          '"Helvetica Neue"',
          "Arial",
          '"PingFang SC"',
          '"Hiragino Sans GB"',
          '"Microsoft YaHei UI"',
          '"Microsoft YaHei"',
          '"Source Han Sans SC"',
          '"Noto Sans CJK SC"',
          "sans-serif",
        ],
        // Mono stack: latin-mono first so digits / log lines stay sharp,
        // then CJK fallbacks for mixed-script log messages.
        mono: [
          '"JetBrains Mono"',
          "ui-monospace",
          "SFMono-Regular",
          '"SF Mono"',
          "Consolas",
          '"Liberation Mono"',
          "Menlo",
          '"Microsoft YaHei UI"',
          '"PingFang SC"',
          "monospace",
        ],
      },
      borderRadius: {
        // Smaller radii than cc-switch (which uses 0.5rem+).
        sm: "2px",
        DEFAULT: "3px",
        md: "4px",
        lg: "6px",
      },
      boxShadow: {
        glow: "0 0 0 1px rgba(91, 231, 196, 0.18), 0 0 12px rgba(91, 231, 196, 0.12)",
      },
      keyframes: {
        // Pulsing LED-style indicator.
        pulse_ring: {
          "0%, 100%": { boxShadow: "0 0 0 0 rgba(91, 231, 196, 0.55)" },
          "50%": { boxShadow: "0 0 0 6px rgba(91, 231, 196, 0)" },
        },
      },
      animation: {
        "pulse-ring": "pulse_ring 1.6s ease-in-out infinite",
      },
    },
  },
  plugins: [],
};
