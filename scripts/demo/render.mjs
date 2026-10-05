// Render the deterministic demo animation to a PNG sequence.
// Usage: node render.mjs [outDir] [fps] [durationSec]
// Requires: puppeteer-core (npm i puppeteer-core) + Chrome installed.
import puppeteer from 'puppeteer-core';
import { mkdirSync } from 'fs';
import { join } from 'path';

const out = process.argv[2] || '/tmp/demo_frames';
const fps = +(process.argv[3] || 60);
const dur = +(process.argv[4] || 14.5);
const CHROME = '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';

mkdirSync(out, { recursive: true });
const browser = await puppeteer.launch({
  executablePath: CHROME,
  headless: 'shell',
  args: ['--hide-scrollbars', '--force-device-scale-factor=1', '--disable-lcd-text'],
});
const page = await browser.newPage();
await page.setViewport({ width: 1920, height: 1200, deviceScaleFactor: 1 });
await page.goto('file://' + new URL('./index.html', import.meta.url).pathname + '#still');
await page.evaluate(() => document.fonts.ready);

const frames = Math.round(dur * fps);
for (let f = 0; f < frames; f++) {
  await page.evaluate(t => window.__setTime(t), f / fps);
  await page.screenshot({ path: join(out, `f${String(f).padStart(4, '0')}.png`) });
  if (f % 60 === 0) console.log(`${f}/${frames}`);
}
await browser.close();
console.log(`done: ${frames} frames -> ${out}`);
