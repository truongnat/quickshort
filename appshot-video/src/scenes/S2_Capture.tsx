import { interpolate, spring, useCurrentFrame, useVideoConfig, staticFile, Img } from "remotion";
import { LandingBackground, AnimatedCursor, Caption, MacWindow, Keycap } from "../components";
import { appConfig } from "../app-config";

// Hotkey -> dimmed overlay -> drag a region. Real app screenshot + live
// crosshair and a growing accent selection rect over it.
export const S2_Capture: React.FC = () => {
  const { brand } = appConfig;
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  // slow push-in over the whole scene
  const zoom = interpolate(frame, [0, 240], [1, 1.07], { extrapolateRight: "clamp" });

  // selection rect grows while cursor drags (frames 60-140 of the scene)
  const sel = interpolate(frame, [62, 138], [0, 1], { extrapolateLeft: "clamp", extrapolateRight: "clamp" });
  const selW = 620 * sel;
  const selH = 420 * sel;
  const selOn = frame >= 62;

  const badge = spring({ frame, fps, delay: 130, config: { mass: 0.6, damping: 14, stiffness: 140 } });

  return (
    <div className="relative flex h-full w-full items-center justify-center overflow-hidden">
      <LandingBackground />

      <div className="relative" style={{ transform: `scale(${zoom})` }}>
        <MacWindow title="Quickshort — overlay" delay={4} scale={0.92}>
          <div className="relative" style={{ width: 1440, height: 810 }}>
            <Img src={staticFile("feat-overlay.png")} style={{ width: "100%", height: "100%", objectFit: "cover" }} />

            {/* live selection accent over the baked screenshot */}
            {selOn && (
              <div
                style={{
                  position: "absolute",
                  left: 420,
                  top: 170,
                  width: selW,
                  height: selH,
                  border: `3px solid ${brand.accent}`,
                  borderRadius: 6,
                  boxShadow: "0 0 0 9999px rgba(0,0,0,0.32)",
                }}
              />
            )}
            <div
              style={{
                position: "absolute",
                left: 420,
                top: 170 + selH + 14,
                opacity: badge,
                transform: `translateY(${(1 - badge) * 8}px)`,
                fontFamily: "ui-monospace, monospace",
                fontSize: 20,
                color: "#ededed",
                background: "#0a0a0a",
                border: "1px solid #262626",
                borderRadius: 6,
                padding: "6px 14px",
              }}
            >
              {Math.round(620 * Math.max(sel, 0.01) * 2)} × {Math.round(420 * Math.max(sel, 0.01) * 2)}
            </div>
          </div>
        </MacWindow>

        <AnimatedCursor
          keyframes={[
            { frame: 30, x: 800, y: 500 },
            { frame: 62, x: 655, y: 385 },
            { frame: 138, x: 965, y: 600 },
          ]}
        />
      </div>

      {/* hotkey chips */}
      <div className="absolute flex" style={{ gap: 16, top: 44, left: 0, right: 0, justifyContent: "center" }}>
        <Keycap label="⌘" delay={18} pressDelay={56} />
        <Keycap label="⇧" delay={24} pressDelay={56} />
        <Keycap label="S" delay={30} pressDelay={56} />
      </div>

      <Caption text="One hotkey. Every monitor dims." delay={150} maxWidth={1200} fontSize={38} />
    </div>
  );
};
