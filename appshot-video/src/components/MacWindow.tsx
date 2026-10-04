import { spring, useCurrentFrame, useVideoConfig } from "remotion";
import { CrossCorners } from "./LandingBackground";

// Landing-language window: hairline frame, flat bar, mono uppercase title,
// + cross marks at the corners (landing motif).
export const MacWindow: React.FC<{
  children: React.ReactNode;
  title?: string;
  delay?: number;
  scale?: number;
  className?: string;
}> = ({ children, title, delay = 0, scale = 1, className }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const enter = spring({
    frame,
    fps,
    delay,
    config: { mass: 1.1, damping: 17, stiffness: 85 },
  });

  return (
    <div
      className={className}
      style={{
        position: "relative",
        opacity: enter,
        transform: `scale(${0.94 + enter * 0.06}) scale(${scale})`,
      }}
    >
      <CrossCorners />
      <div
        className="overflow-hidden"
        style={{ borderRadius: 8, border: "1px solid #262626", background: "#0a0a0a" }}
      >
        <div
          className="relative flex items-center px-4"
          style={{ height: 44, borderBottom: "1px solid #1a1a1a" }}
        >
          <div className="flex" style={{ gap: 7 }}>
            <div className="rounded-full" style={{ width: 12, height: 12, background: "#FF5F57" }} />
            <div className="rounded-full" style={{ width: 12, height: 12, background: "#FEBC2E" }} />
            <div className="rounded-full" style={{ width: 12, height: 12, background: "#28C840" }} />
          </div>
          {title && (
            <span
              className="absolute left-0 right-0 text-center font-mono"
              style={{ fontSize: 12, letterSpacing: "0.14em", color: "#6b6b6b", pointerEvents: "none" }}
            >
              {title.toUpperCase()}
            </span>
          )}
        </div>
        <div className="relative overflow-hidden" style={{ background: "#000" }}>
          {children}
        </div>
      </div>
    </div>
  );
};
