import { interpolate, spring, useCurrentFrame, useVideoConfig, staticFile, Img } from "remotion";
import { AmbientBackground, Caption, FadeIn, MacWindow } from "../components";
import { appConfig } from "../app-config";

// Annotate: real annotated capture + a live arrow that draws itself +
// an "editable block" outline that pulses around a shape.
export const S3_Annotate: React.FC = () => {
  const { brand } = appConfig;
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const zoom = interpolate(frame, [0, 210], [1, 1.08], { extrapolateRight: "clamp" });

  // arrow draws itself frames 40-75
  const draw = interpolate(frame, [40, 78], [620, 0], { extrapolateLeft: "clamp", extrapolateRight: "clamp" });

  // editable-block outline appears at 100, pulses gently
  const blockOn = spring({ frame, fps, delay: 100, config: { mass: 0.6, damping: 12, stiffness: 130 } });
  const pulse = interpolate(Math.sin(Math.max(0, frame - 110) * 0.12), [-1, 1], [0.5, 1]);

  return (
    <div className="relative flex h-full w-full items-center justify-center overflow-hidden">
      <AmbientBackground brand={brand} variant="dark" />

      <div className="relative" style={{ transform: `scale(${zoom})` }}>
        <MacWindow title="annotation" delay={4} scale={0.92}>
          <div className="relative" style={{ width: 1440, height: 810 }}>
            <Img src={staticFile("feat-annotate.png")} style={{ width: "100%", height: "100%", objectFit: "cover" }} />

            {/* arrow draws itself */}
            <svg
              viewBox="0 0 300 200"
              style={{ position: "absolute", left: 950, top: 130, width: 300, height: 200, overflow: "visible" }}
            >
              <path
                d="M 280 24 C 200 62 120 112 46 166 M 46 166 L 102 144 M 46 166 L 66 110"
                stroke="#ff5f57"
                strokeWidth={9}
                fill="none"
                strokeLinecap="round"
                strokeDasharray={620}
                strokeDashoffset={draw}
              />
            </svg>

            {/* editable block handles around the big red box in the shot */}
            <div
              style={{
                position: "absolute",
                left: 285,
                top: 455,
                width: 430,
                height: 190,
                border: `3px dashed ${brand.accent}`,
                borderRadius: 8,
                opacity: blockOn * pulse,
                boxShadow: `0 0 24px ${brand.accent}55`,
              }}
            >
              {[[-9, -9], [421, -9], [-9, 181], [421, 181]].map(([x, y], i) => (
                <div
                  key={i}
                  style={{
                    position: "absolute", left: x, top: y, width: 18, height: 18,
                    borderRadius: 4, background: "#0a0a0a", border: `2px solid ${brand.accent}`,
                  }}
                />
              ))}
            </div>
          </div>
        </MacWindow>
      </div>

      <Caption text="Every block stays editable — move, restyle, undo." delay={110} maxWidth={1300} fontSize={38} />
    </div>
  );
};
