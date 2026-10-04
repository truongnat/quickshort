import { AbsoluteFill, Audio, Sequence, Series, staticFile } from "remotion";
import { SceneWrap } from "./components";
import { appConfig } from "./app-config";
import { S1_Hook } from "./scenes/S1_Hook";
import { S2_Capture } from "./scenes/S2_Capture";
import { S3_Annotate } from "./scenes/S3_Annotate";
import { S4_Share } from "./scenes/S4_Share";
import { S5_Settings } from "./scenes/S5_Settings";
import { S6_CTA } from "./scenes/S6_CTA";

const SCENES: { dur: number; el: React.ReactElement }[] = [
  { dur: 120, el: <S1_Hook /> },
  { dur: 240, el: <S2_Capture /> },
  { dur: 210, el: <S3_Annotate /> },
  { dur: 180, el: <S4_Share /> },
  { dur: 180, el: <S5_Settings /> },
  { dur: 150, el: <S6_CTA /> },
];

export const DEMO_DURATION = SCENES.reduce((a, s) => a + s.dur, 0); // 1080f = 36s

export const ProductDemo: React.FC = () => {
  return (
    <AbsoluteFill style={{ background: "#000", fontFamily: "Inter, system-ui, sans-serif" }}>
      {appConfig.video.backgroundMusic && (
        <Sequence durationInFrames={DEMO_DURATION}>
          <Audio
            src={staticFile(appConfig.video.backgroundMusic)}
            volume={appConfig.video.backgroundMusicVolume}
            endAt={DEMO_DURATION}
          />
        </Sequence>
      )}
      <Series>
        {SCENES.map((s, i) => (
          <Series.Sequence key={i} durationInFrames={s.dur}>
            <SceneWrap
              durationInFrames={s.dur}
              fadeIn={i !== 0}
              fadeOut={i !== SCENES.length - 1}
            >
              {s.el}
            </SceneWrap>
          </Series.Sequence>
        ))}
      </Series>
    </AbsoluteFill>
  );
};
