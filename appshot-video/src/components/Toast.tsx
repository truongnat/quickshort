import { spring, useCurrentFrame, useVideoConfig } from "remotion";

// Quickshort-style toast: glass card, green tick, slides up.
export const Toast: React.FC<{
  children: React.ReactNode;
  delay?: number;
}> = ({ children, delay = 0 }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();
  const enter = spring({ frame, fps, delay, config: { mass: 0.8, damping: 13, stiffness: 140 } });

  return (
    <div
      className="flex items-center"
      style={{
        gap: 14,
        padding: "18px 30px",
        borderRadius: 16,
        background: "rgba(16,16,16,0.94)",
        border: "1px solid #2e2e2e",
        backdropFilter: "blur(20px)",
        boxShadow: "0 24px 60px rgba(0,0,0,0.55)",
        fontSize: 26,
        color: "#ededed",
        opacity: enter,
        transform: `translateY(${(1 - enter) * 26}px)`,
      }}
    >
      {children}
    </div>
  );
};
