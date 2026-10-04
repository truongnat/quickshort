import { interpolate, spring, useCurrentFrame, useVideoConfig } from "remotion";
import { AmbientBackground, AppIcon, FadeIn } from "../components";
import { appConfig } from "../app-config";

export const S6_CTA: React.FC = () => {
  const { brand } = appConfig;
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const btnEnter = spring({ frame, fps, delay: 28, config: { mass: 1.1, damping: 13, stiffness: 95 } });
  const pulse = interpolate(Math.sin(frame * 0.09), [-1, 1], [0.97, 1.03]);

  return (
    <div className="relative flex h-full w-full flex-col items-center justify-center overflow-hidden">
      <AmbientBackground brand={brand} variant="dark" />
      <div className="relative z-10 flex flex-col items-center" style={{ gap: 30 }}>
        <AppIcon src={appConfig.app.icon} size={110} glow glowColor={`${brand.primary}55`} />
        <FadeIn delay={8} distance={22}>
          <span style={{ fontSize: 68, fontWeight: 800, letterSpacing: "-0.03em", color: brand.textPrimary }}>
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
                  fontSize: 15,
                  letterSpacing: "0.2em",
                  color: brand.textSecondary,
                  border: "1px solid #2e2e2e",
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
            marginTop: 12,
            padding: "18px 44px",
            borderRadius: 14,
            background: brand.primary,
            fontSize: 26,
            fontWeight: 700,
            color: "#04161c",
          }}
        >
          brew install --cask quickshort
        </div>
        <FadeIn delay={40} distance={10}>
          <span className="font-mono" style={{ fontSize: 19, color: brand.textSecondary }}>
            {appConfig.app.url}
          </span>
        </FadeIn>
      </div>
    </div>
  );
};
