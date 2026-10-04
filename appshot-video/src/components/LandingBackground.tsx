import { interpolate, useCurrentFrame } from "remotion";

// Landing-page background: pure black + dashed hairline grid +
// bright dash segments running along the lines (runV/runH motif).
export const LandingBackground: React.FC = () => {
  const frame = useCurrentFrame();

  // running dashes — positions loop over ~8s
  const vRun = interpolate(frame % 240, [0, 240], [-12, 112]);
  const hRun = interpolate(frame % 300, [0, 300], [-12, 112]);

  const line = "1px dashed #232323";

  return (
    <div className="absolute inset-0" style={{ background: "#000" }}>
      {/* vertical dashed lines */}
      {[18, 50, 82].map((x) => (
        <div key={x} style={{ position: "absolute", top: 0, bottom: 0, left: `${x}%`, borderLeft: line }} />
      ))}
      {/* horizontal dashed lines */}
      {[22, 78].map((y) => (
        <div key={y} style={{ position: "absolute", left: 0, right: 0, top: `${y}%`, borderTop: line }} />
      ))}
      {/* running bright segment — vertical */}
      <div
        style={{
          position: "absolute", left: "50%", width: 1, top: `${vRun}%`, height: 110,
          background: "linear-gradient(180deg, transparent, rgba(255,255,255,0.5), transparent)",
        }}
      />
      {/* running bright segment — horizontal */}
      <div
        style={{
          position: "absolute", top: "22%", height: 1, left: `${hRun}%`, width: 160,
          background: "linear-gradient(90deg, transparent, rgba(255,255,255,0.45), transparent)",
        }}
      />
      {/* faint center vignette so content pops */}
      <div
        style={{
          position: "absolute", inset: 0,
          background: "radial-gradient(1100px 620px at 50% 46%, rgba(82,199,234,0.045), transparent 70%)",
        }}
      />
    </div>
  );
};

// + cross marks at the four corners of a framed element (landing motif).
export const CrossCorners: React.FC<{ color?: string; inset?: number }> = ({
  color = "#6b6b6b",
  inset = -9,
}) => (
  <>
    {[
      { top: inset, left: inset },
      { top: inset, right: inset },
      { bottom: inset, left: inset },
      { bottom: inset, right: inset },
    ].map((pos, i) => (
      <span
        key={i}
        className="font-mono"
        style={{
          position: "absolute",
          ...pos,
          width: 17,
          height: 17,
          fontSize: 15,
          lineHeight: "17px",
          textAlign: "center",
          color,
          pointerEvents: "none",
          zIndex: 5,
        }}
      >
        +
      </span>
    ))}
  </>
);
