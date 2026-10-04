import { spring, useCurrentFrame, useVideoConfig } from "remotion";

// macOS-style dark window chrome: traffic lights + centered title.
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
    config: { mass: 1.2, damping: 16, stiffness: 80 },
  });

  return (
    <div
      className={className}
      style={{
        opacity: enter,
        transform: `scale(${0.92 + enter * 0.08}) scale(${scale})`,
      }}
    >
      <div
        className="overflow-hidden"
        style={{
          borderRadius: 14,
          boxShadow:
            "0 40px 120px rgba(0,0,0,0.55), 0 0 0 1px rgba(255,255,255,0.08)",
          border: "1px solid #262626",
        }}
      >
        <div
          className="relative flex items-center px-4"
          style={{
            height: 40,
            background: "#161616",
            borderBottom: "1px solid #262626",
          }}
        >
          <div className="flex" style={{ gap: 7 }}>
            <div className="rounded-full" style={{ width: 12, height: 12, background: "#FF5F57" }} />
            <div className="rounded-full" style={{ width: 12, height: 12, background: "#FEBC2E" }} />
            <div className="rounded-full" style={{ width: 12, height: 12, background: "#28C840" }} />
          </div>
          {title && (
            <span
              className="absolute left-0 right-0 text-center font-mono"
              style={{ fontSize: 13, color: "#a1a1a1", pointerEvents: "none" }}
            >
              {title}
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
