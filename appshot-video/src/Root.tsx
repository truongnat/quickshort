import { Composition } from "remotion";
import { appConfig } from "./app-config";
import { ProductDemo, DEMO_DURATION } from "./ProductDemo";
import "./styles.css";

export const RemotionRoot: React.FC = () => {
  return (
    <Composition
      id="ProductDemo"
      component={ProductDemo}
      durationInFrames={DEMO_DURATION}
      fps={appConfig.video.fps}
      width={appConfig.video.width}
      height={appConfig.video.height}
    />
  );
};
