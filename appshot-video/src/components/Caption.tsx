import { spring, useCurrentFrame, useVideoConfig } from "remotion";

// Landing-language caption: hairline pill, accent dot, word-by-word spring.
export const Caption: React.FC<{
  text: string;
  delay?: number;
  fontSize?: number;
  maxWidth?: number;
}> = ({ text, delay = 0, fontSize = 34, maxWidth = 1100 }) => {
  const frame = useCurrentFrame();
  const { fps } = useVideoConfig();

  const containerEnter = spring({
    frame,
    fps,
    delay,
    config: { mass: 0.8, damping: 15, stiffness: 110 },
  });

  const words = text.split(" ");

  return (
    <div
      className="absolute bottom-0 left-0 right-0 z-50 flex justify-center pb-32"
      style={{ opacity: containerEnter, transform: `translateY(${(1 - containerEnter) * 14}px)` }}
    >
      <div
        className="flex items-center rounded-full"
        style={{
          gap: 16,
          padding: "16px 34px",
          background: "rgba(0,0,0,0.88)",
          border: "1px solid #262626",
          maxWidth,
        }}
      >
        <span
          className="rounded-full"
          style={{ width: 9, height: 9, background: "#52c7ea", flexShrink: 0 }}
        />
        <p
          className="text-center leading-tight"
          style={{ fontSize, fontWeight: 550, letterSpacing: "-0.015em", color: "#ededed" }}
        >
          {words.map((word, i) => {
            const wordEnter = spring({
              frame,
              fps,
              delay: delay + 3 + i * 1.5,
              config: { mass: 0.3, damping: 12, stiffness: 150 },
            });
            return (
              <span
                key={i}
                className="inline-block"
                style={{
                  opacity: wordEnter,
                  transform: `translateY(${(1 - wordEnter) * 8}px)`,
                  marginRight: "0.3em",
                }}
              >
                {word}
              </span>
            );
          })}
        </p>
      </div>
    </div>
  );
};
