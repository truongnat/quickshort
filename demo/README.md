# demo/

Scripted product demo, rendered as a pure CSS keyframe timeline
(`index.html`, ~18s at 1600x900) and recorded headless.

Regenerate:

```bash
npx playwright install ffmpeg   # once — playwright needs its own ffmpeg
NODE_PATH=$(npm root -g) node demo/record.mjs
ffmpeg -i demo/out/*.webm -vf "crop=1600:812:0:0" -c:v libx264 -pix_fmt yuv420p -crf 20 -movflags +faststart docs/assets/demo.mp4
ffmpeg -i demo/out/*.webm -vf "crop=1600:812:0:0" -c:v libvpx-vp9 -b:v 0 -crf 32 docs/assets/demo.webm
```

The crop trims a dead gray band Chrome adds at the bottom of
screencast frames. Assets land in `docs/assets/` and are embedded in
the landing `#showcase` section.
