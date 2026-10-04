import { spring, useCurrentFrame, useVideoConfig, interpolate } from "remotion";
import { AmbientBackground, AppIcon, Caption, FadeIn } from "../components";
import { appConfig } from "../app-config";

export const S1_Hook: React.FC = () => {
  const { brand } = appConfig;
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const iconEnter = spring({ frame, fps, delay: 0, config: { mass: 0.9, damping: 13, stiffness: 110 } });
  const breathe = interpolate(Math.sin(frame * 0.05), [-1, 1], [0.98, 1.03]);

  return (
    <div className="relative flex h-full w-full flex-col items-center justify-center overflow-hidden">
      <AmbientBackground brand={brand} variant="dark" />
      <div className="relative z-10 flex flex-col items-center">
        <div style={{ opacity: iconEnter, transform: `scale(${breathe})` }}>
          <AppIcon src={appConfig.app.icon} size={130} glow glowColor={`${brand.primary}55`} />
        </div>
        <FadeIn delay={10} distance={26}>
          <span style={{ fontSize: 76, fontWeight: 800, letterSpacing: "-0.03em", color: brand.textPrimary }}>
            {appConfig.app.name}
          </span>
        </FadeIn>
        <FadeIn delay={20} distance={18}>
          <span className="font-mono" style={{ fontSize: 26, color: brand.primary, letterSpacing: "0.08em", marginTop: 14 }}>
            {appConfig.app.tagline}
          </span>
        </FadeIn>
      </div>
      <Caption text="Screenshots, minus the ceremony." delay={34} maxWidth={1200} fontSize={38} />
    </div>
  );
};
