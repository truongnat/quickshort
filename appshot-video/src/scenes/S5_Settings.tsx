import { interpolate, spring, useCurrentFrame, useVideoConfig, staticFile, Img } from "remotion";
import { AmbientBackground, Caption, FadeIn, MacWindow } from "../components";
import { appConfig } from "../app-config";

// Settings window + self-update proof point.
export const S5_Settings: React.FC = () => {
  const { brand } = appConfig;
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const zoom = interpolate(frame, [0, 180], [1, 1.06], { extrapolateRight: "clamp" });
  const pill = spring({ frame, fps, delay: 80, config: { mass: 0.7, damping: 12, stiffness: 120 } });

  return (
    <div className="relative flex h-full w-full items-center justify-center overflow-hidden">
      <AmbientBackground brand={brand} variant="dark" />

      <div className="relative" style={{ transform: `scale(${zoom}) translateY(-70px)` }}>
        <MacWindow title="Quickshort — Settings" delay={4} scale={0.82}>
          <div className="relative" style={{ width: 1200, height: 780 }}>
            <Img src={staticFile("feat-settings.png")} style={{ width: "100%", height: "100%", objectFit: "cover" }} />
          </div>
        </MacWindow>

        {/* update pill floating on the corner */}
        <div
          className="absolute flex items-center font-mono"
          style={{
            right: -30,
            top: -34,
            gap: 12,
            padding: "14px 24px",
            borderRadius: 999,
            background: "rgba(16,16,16,0.96)",
            border: `1px solid ${brand.accent}55`,
            fontSize: 20,
            color: brand.textPrimary,
            opacity: pill,
            transform: `translateY(${(1 - pill) * 16}px)`,
            boxShadow: `0 18px 50px rgba(0,0,0,0.5), 0 0 30px ${brand.accent}33`,
          }}
        >
          <span style={{ color: brand.success }}>↻</span> checks releases daily · swaps in place
        </div>
      </div>

      <Caption text="It updates itself. Permissions survive." delay={95} maxWidth={1200} fontSize={38} />
    </div>
  );
};
