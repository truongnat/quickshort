import { spring, useCurrentFrame, useVideoConfig } from "remotion";

const TONES = {
  ok: { color: "#30d158", border: "rgba(48,209,88,.45)" },
  info: { color: "#52c7ea", border: "rgba(82,199,234,.45)" },
  err: { color: "#ff5f57", border: "rgba(255,95,87,.45)" },
  work: { color: "#52c7ea", border: "rgba(82,199,234,.45)" },
} as const;

// Landing-language toast: flat #0a0a0a, hairline border,
// square icon tile tinted by tone, title + mono sub.
export const Toast: React.FC<{
  icon: string;
  tone?: keyof typeof TONES;
  title: string;
  sub?: string;
  delay?: number;
}> = ({ icon, tone = "info", title, sub, delay = 0 }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const enter = spring({ frame, fps, delay, config: { mass: 0.7, damping: 14, stiffness: 150 } });

  const t = TONES[tone];
  const spinning = tone === "work";

  return (
    <div
      className="flex items-center"
      style={{
        gap: 16,
        padding: "14px 26px 14px 14px",
        borderRadius: 8,
        background: "#0a0a0a",
        border: "1px solid #262626",
        opacity: enter,
        transform: `translateY(${(1 - enter) * 14}px)`,
      }}
    >
      <div
        className="flex items-center justify-center font-mono"
        style={{
          width: 34,
          height: 34,
          borderRadius: 6,
          border: `1px solid ${t.border}`,
          color: t.color,
          fontSize: 15,
          flexShrink: 0,
          animation: spinning ? "spin 1.2s linear infinite" : undefined,
        }}
      >
        {icon}
      </div>
      <div>
        <div style={{ fontSize: 17, fontWeight: 550, letterSpacing: "-0.01em", color: "#ededed" }}>
          {title}
        </div>
        {sub && (
          <div className="font-mono" style={{ fontSize: 12.5, color: "#a1a1a1", marginTop: 3 }}>
            {sub}
          </div>
        )}
      </div>
    </div>
  );
};
