import { spring, useCurrentFrame, useVideoConfig } from "remotion";

// Hardware keycap chip — pops in, dips on "press".
export const Keycap: React.FC<{
  label: string;
  delay?: number;
  pressDelay?: number;
}> = ({ label, delay = 0, pressDelay }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const enter = spring({ frame, fps, delay, config: { mass: 0.6, damping: 12, stiffness: 160 } });
  const pressed = pressDelay !== undefined && frame >= pressDelay ? 1 : 0;

  return (
    <div
      className="flex items-center justify-center font-mono"
      style={{
        minWidth: 74,
        height: 74,
        padding: "0 20px",
        fontSize: 30,
        color: "#ededed",
        background: "rgba(22,22,22,0.97)",
        border: "1px solid #3a3a3a",
        borderBottomWidth: pressed ? 2 : 5,
        borderRadius: 14,
        opacity: enter,
        transform: `translateY(${(1 - enter) * 18 + pressed * 3}px) scale(${0.85 + enter * 0.15 - pressed * 0.04})`,
        boxShadow: "0 14px 40px rgba(0,0,0,0.6)",
      }}
    >
      {label}
    </div>
  );
};
