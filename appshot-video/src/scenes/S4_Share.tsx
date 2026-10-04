import { interpolate, spring, useCurrentFrame, useVideoConfig, staticFile, Img } from "remotion";
import { LandingBackground, Caption, Keycap, Toast, CrossCorners } from "../components";
import { appConfig } from "../app-config";

// Save -> copy -> share link. Capture card in front, toasts fire on beats.
export const S4_Share: React.FC = () => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const cardEnter = spring({ frame, fps, delay: 0, config: { mass: 1, damping: 15, stiffness: 90 } });
  const cardFloat = interpolate(Math.sin(frame * 0.045), [-1, 1], [-8, 8]);

  const t1in = frame >= 55 && frame < 118;
  const t2in = frame >= 122;

  return (
    <div className="relative flex h-full w-full items-center justify-center overflow-hidden">
      <LandingBackground />

      {/* the capture card, floating */}
      <div
        style={{
          position: "relative",
          opacity: cardEnter,
          transform: `translateY(${(1 - cardEnter) * 40 + cardFloat}px) scale(${0.9 + cardEnter * 0.1})`,
        }}
      >
        <CrossCorners />
        <div
          className="overflow-hidden"
          style={{
            width: 980,
            borderRadius: 8,
            border: "1px solid #262626",
            background: "#0a0a0a",
            padding: 10,
          }}
        >
          <Img src={staticFile("screenshot.png")} style={{ width: "100%", display: "block", borderRadius: 4 }} />
        </div>
      </div>

      {/* ⌘S then ⌘L chips */}
      {frame >= 30 && frame < 118 && (
        <div className="absolute flex" style={{ gap: 14, top: 120, left: 0, right: 0, justifyContent: "center" }}>
          <Keycap label="⌘" delay={30} pressDelay={48} />
          <Keycap label="S" delay={36} pressDelay={48} />
        </div>
      )}
      {t2in && (
        <div className="absolute flex" style={{ gap: 14, top: 120, left: 0, right: 0, justifyContent: "center" }}>
          <Keycap label="⌘" delay={0} pressDelay={12} />
          <Keycap label="L" delay={4} pressDelay={12} />
        </div>
      )}

      {/* toasts bottom-center */}
      <div className="absolute" style={{ bottom: 268, left: 0, right: 0, display: "flex", justifyContent: "center" }}>
        {t1in && (
          <Toast
            icon="✓"
            tone="ok"
            title="Saved & copied"
            sub="quickshort-1791.png · 1.2 MB"
            delay={55}
          />
        )}
        {t2in && (
          <Toast
            icon="⧉"
            tone="info"
            title="Link copied"
            sub="quickshort.tunnl.gg/s/9f2ac1 · expires on quit"
            delay={0}
          />
        )}
      </div>

      <Caption text="Saved, copied, shared — one keystroke each." delay={70} maxWidth={1300} fontSize={38} />
    </div>
  );
};
