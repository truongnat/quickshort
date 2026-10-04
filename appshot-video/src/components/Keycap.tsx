import { spring, useCurrentFrame, useVideoConfig } from "remotion";

// Landing-language keycap: flat hairline tile.
// Pressed = cyan text + cyan tint bg + cyan border (active-tab pattern).
export const Keycap: React.FC<{
  label: string;
  delay?: number;
  pressDelay?: number;
}> = ({ label, delay = 0, pressDelay }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const enter = spring({ frame, fps, delay, config: { mass: 0.6, damping: 13, stiffness: 160 } });
  const pressed = pressDelay !== undefined && frame >= pressDelay;

  return (
    <div
      className="flex items-center justify-center font-mono"
      style={{
        minWidth: 66,
        height: 66,
        padding: "0 18px",
        fontSize: 25,
        color: pressed ? "#52c7ea" : "#ededed",
        background: pressed ? "rgba(82,199,234,0.08)" : "#0a0a0a",
        border: `1px solid ${pressed ? "rgba(82,199,234,0.55)" : "#262626"}`,
        borderRadius: 8,
        opacity: enter,
        transform: `translateY(${(1 - enter) * 14}px)`,
      }}
    >
      {label}
    </div>
  );
};
