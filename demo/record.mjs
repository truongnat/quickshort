// Record demo/index.html -> demo/out/*.webm using the installed Chrome.
// NODE_PATH=$(npm root -g) node demo/record.mjs
import { createRequire } from "module";
import { fileURLToPath } from "url";
import path from "path";

const { chromium } = createRequire(import.meta.url)("@playwright/test");

const here = path.dirname(fileURLToPath(import.meta.url));
const pageUrl = "file://" + path.join(here, "index.html");
const outDir = path.join(here, "out");

const browser = await chromium.launch({ channel: "chrome", headless: true });
const ctx = await browser.newContext({
  viewport: { width: 1600, height: 900 },
  deviceScaleFactor: 1,
  recordVideo: { dir: outDir, size: { width: 1600, height: 900 } },
});
const page = await ctx.newPage();
await page.goto(pageUrl);
await page.waitForTimeout(17500); // full timeline + tail
const video = page.video();
await ctx.close();
await browser.close();
console.log(await video.path());
