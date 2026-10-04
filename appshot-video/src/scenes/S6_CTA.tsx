import { interpolate, spring, useCurrentFrame, useVideoConfig } from "remotion";
import { LandingBackground, AppIcon, FadeIn } from "../components";
import { appConfig } from "../app-config";

export const S6_CTA: React.FC = () => {
  const { brand } = appConfig;
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const btnEnter = spring({ frame, fps, delay: 28, config: { mass: 1.1, damping: 14, stiffness: 100 } });
  const pulse = interpolate(Math.sin(frame * 0.07), [-1, 1], [0.985, 1.015]);

  return (
    <div className="relative flex h-full w-full flex-col items-center justify-center overflow-hidden">
      <LandingBackground />
      <div className="relative z-10 flex flex-col items-center" style={{ gap: 32 }}>
        <AppIcon src={appConfig.app.icon} size={110} glow glowColor="rgba(82,199,234,0.33)" />
        <FadeIn delay={8} distance={22}>
          <span style={{ fontSize: 72, fontWeight: 800, letterSpacing: "-0.035em", color: brand.textPrimary }}>
            {appConfig.app.name}
          </span>
        </FadeIn>
        <FadeIn delay={16} distance={16}>
          <div className="flex" style={{ gap: 12 }}>
            {["NATIVE", "PURE RUST", "NO ELECTRON"].map((c) => (
              <span
                key={c}
                className="font-mono"
                style={{
                  fontSize: 13,
                  letterSpacing: "0.22em",
                  color: "#a1a1a1",
                  border: "1px solid #262626",
                  borderRadius: 999,
                  padding: "8px 18px",
                }}
              >
                {c}
              </span>
            ))}
          </div>
        </FadeIn>
        <div
          className="font-mono"
          style={{
            opacity: btnEnter,
            transform: `scale(${pulse})`,
            marginTop: 10,
            padding: "17px 44px",
            borderRadius: 999,
            background: "#ededed",
            fontSize: 22,
            fontWeight: 600,
            color: "#000",
          }}
        >
          brew install --cask quickshort
        </div>
        <FadeIn delay={40} distance={10}>
          <span
            className="font-mono"
            style={{
              fontSize: 17,
              color: "#a1a1a1",
              border: "1px solid #262626",
              borderRadius: 999,
              padding: "9px 22px",
            }}
          >
            {appConfig.app.url}
          </span>
        </FadeIn>
      </div>
    </div>
  );
};
