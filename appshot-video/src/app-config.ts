import type { AppConfig } from "./config";

export const appConfig: AppConfig = {
  app: {
    name: "Quickshort",
    tagline: "Select. Snip. Ship.",
    icon: "icon.png",
    platform: "web",
    url: "github.com/truongnat/quickshort",
  },
  brand: {
    primary: "#52c7ea",
    primaryLight: "#0d2a33",
    background: "#000000",
    surface: "#0a0a0a",
    textPrimary: "#ededed",
    textSecondary: "#a1a1a1",
    success: "#30d158",
    danger: "#ff5f57",
    accent: "#52c7ea",
  },
  video: {
    fps: 30,
    width: 1920,
    height: 1080,
    browser: "chrome-dark",
    backgroundMusic: "music/cinematic-build.mp3",
    backgroundMusicVolume: 0.28,
  },
};
