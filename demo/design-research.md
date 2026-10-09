# Quickshort — Design Research

Teardown of screenshot-tool competitors + dev-tool landing patterns, to sharpen Quickshort's design language across three surfaces: **landing page** (`docs/`), **in-app UI**, and the **Remotion demo video** (`appshot-video/`). Baseline: current landing (`docs/index.html`, `docs/style.css`) and the `demo/ui-2026.html` kit already run a Vercel/Linear-style engineering aesthetic — pure black `#000`, hairline `#262626` borders, dashed grid lines, "+" corner marks, Geist + Geist Mono, accent `#52c7ea`.

---

## 1. TL;DR — recommendations

- **Keep the engineering aesthetic, but differentiate inside it.** The Vercel-clone look (Geist, hairlines, "+" crosses, mono labels) is now a codified, mass-produced template genre — SaaS Design ships it as a starter, AI builders generate it from prompts. It still signals "developer tool," but it no longer differentiates. Quickshort's edge should be **product-embedded motifs**: selection rectangles, `W×H` size counters, keycaps, crosshair ticks — the app's own capture vocabulary — instead of generic grid furniture.
- **Show the loop, not the inventory.** CleanShot and Screen Studio win by making the *product state* carry each section (one big believable mockup, or a per-feature video with tab switchers). Quickshort's showcase tab-switcher (DEMO/OVERLAY/ANNOTATE/TRAY/SETTINGS) is already the right instinct — extend that pattern instead of the current 12-cell feature wall, which reads like ShareX's taxonomy dump.
- **Lead with numbers only a native Rust tool can claim.** Shottr sells itself with "2.3 MB dmg · 17 ms to grab." Quickshort has equivalents: binary size, "zero Electron," physical-pixel Retina output, 15 fps recording, link expiry. Put a measurable claim in the hero sub, not just in body copy.
- **Free/OSS is the positioning vs. the whole paid field.** Every serious Mac competitor charges ($29–$89 one-time or $39/yr). Quickshort's "free, open-source, no account, no analytics" should be a hero-level trust signal, framed like ShareX ("free forever, open source, no account") not buried in a features cell.
- **In-app: one-key-per-tool + floating result card.** The category convention is clear — tools on single keys (Shottr: 16 tools; Snapok: R/O/A/P/M/T/V), a size counter at the selection, and a post-capture floating card (CleanShot's Quick Access Overlay) instead of a plain toast. Quickshort's ui-2026 toast design is already on-brand; promote it from a notification to a mini action card (save / copy / pin / share buttons on it).
- **Demo video: steal Screen Studio's grammar.** Per-feature segments where a UI element zooms in with smooth motion, cursor-path smoothing, click ripples, and a mono caption strip naming what's happening. Render keycaps + selection-rect callouts in the landing's exact tokens so the video is recognizable as the same product.
- **One accent, spent sparingly.** CleanShot's design teardown: "keep blue for primary intent — when everything turns blue, the system loses its clarity." `#52c7ea` should mean *interactive/active* (focus ring, active tool, pressed key, running dash), never decoration. Add a second "capture" signal color for the selection rectangle itself so the app reads correctly on any content.

---

## 2. Competitor teardown table

| Competitor | Site vibe | Dominant pattern | What to steal | What to avoid |
|---|---|---|---|---|
| **CleanShot X** — cleanshot.com | Polished Mac-native premium. **Light theme**: wide white space, black headlines, single blue pill CTA; Inter typeface. "There are average capture apps. And there's CleanShot." | Screenshot-led. One large product mockup per feature section; sticky CTA; bento for smaller features; testimonials scattered *relevant to each section* plus a 350+-tweet wall | Social-proof placement (testimonial beside the feature it praises); logo wall "Trusted by teams at: Panic, Linear, Vercel, Cursor, 1Password"; "Trusted by 250,000+ users" under the H1; product mockups doing the storytelling, chrome kept quiet | The confidence game only works with their reputation — don't ape the "we're the best" swagger with zero users. Light theme off-brand for Quickshort |
| **Shottr** — shottr.cc | Indie-craft, honest, fast. Light, clean, single-dev personality ("Don't worry, I'm too lazy for spam") | Rotating-audience hero ("for designers / front-end engineers / those who care about pixels"); dense feature sections; huge Tips & Tricks shortcut compendium | **Spec-as-marketing**: "2.3 MB dmg, 17 ms to grab, ~165 ms to show." Numbers beat adjectives for pixel-people. Keyboard-everything depth signals craft. In-app: 16 tools in the toolbar, properties panel top-right, clickable dimension box toggles logical/physical px | Wall-of-features density without hierarchy; the page is long and undifferentiated section-to-section |
| **Snagit** — techsmith.com/snagit (marketing lander: snagit.webflow.io) | Enterprise incumbent, salesy. Serif italic headlines, cheeky tone ("You're not *still using* snipping tool…right?"), comparison-table-driven | Classic B2B landing: pain-point hero → feature carousel → testimonial carousel → **vs-built-in comparison table** → pricing with objection handling ("Yes, Snagit costs money. So do all the hours you've lost") | The **"vs built-in tools" comparison table** concept — Quickshort vs. macOS screenshot vs. Snipping Tool is a legitimate, persuasive section for a free tool. Objection-handling copy pattern | Everything else: $39/yr subscription framing, salesy tone, checklist-green checkmarks, "100% of Fortune 500" enterprise posturing |
| **Monosnap** — monosnap.com / monosnap.ai | Dated freemium cloud tool; site is a JS SPA with little visible design language | Pricing-tiered product (Free / Non-Commercial / Commercial); cloud-first (login, 2 GB cloud) | Freemium table clarity — users instantly see what's free | Cloud-account requirement as the product's center of gravity; JS-heavy marketing shell with no personality |
| **Zappy** — zapier.com/zappy | Bare-bones corporate freebie; minimal Zapier-hosted page | **Numbered 4-step flow**: "1. Capture with a shortcut → 2. Annotate → 3. Press enter → 4. Share as URL" | The numbered-loop narrative — the whole product explained as one sequential flow (maps to Quickshort's pipeline section). Cheap, clear, fast to build | Treating it as an afterthought page; cloud upload gated behind a paid Zapier account |
| **Flameshot** — flameshot.org | Functional OSS project site; competent but generic | Feature paragraphs + one annotated in-app screenshot; **per-OS download matrix with package-manager commands** (`pacman -S flameshot`, `apt install`…); sponsor logos (JetBrains, NameCheap, SignPath); GPLv3/OSI badges | Per-OS install-command blocks (Quickshort already does this); license/OSI badges as instant trust marks for OSS users | Feature copy in uniform blocks with no visual hierarchy; site reads "documentation," not "product" |
| **ShareX** — getsharex.com | Power-user institution. Dark blue brand; dense information architecture | Credibility stats up top ("33M downloads, 18 years, privacy first, no ads"); exhaustive feature taxonomy (capture methods / after-capture tasks / upload methods / tools) | **"After capture tasks"** as a named concept — Quickshort's save/copy/pin/share/OCR actions are exactly this; framing it as a workflow stage is honest and resonates with power users. Big-number trust stats | Feature taxonomy sprawl — dozens of bullets in flat lists, no prioritization. The page proves power but can't sell it |
| **Kap** — getkap.co | Minimal, friendly, airy. White/light, huge window mockup, generous whitespace | Single-screen product shot; **plugin cards row** (Giphy/Vercel/Streamable exporters); "Loved by the community" logo strip (Vercel, GitHub, Sentry, Google…) | Restraint — the whole pitch in ~5 sections. Community/contributor wall pattern that maps well to Quickshort's sponsor wall. "Built with web technology" honesty = our "pure Rust" line | Almost too minimal — few features surfaced; works for a simple recorder, undersells a feature-dense tool |
| **Xnapper** — xnapper.com | Playful, colorful, creator-focused. Gradient-heavy product shots, emoji-ish energy, awards laurels | Benefit-led hero ("Snap Beautiful Screenshots Instantly — no design skills? No problem. Snap → Preview → Share"); **"automatic" feature framing** (auto-balance, auto background, auto-redact); awards badges + "4.9 stars · 4,000 creators"; free + one-time watermark removal | **Auto-everything framing for Beautify**: Quickshort's beautify is manual-adjacent; the pitch "balanced, backdrop, shadow — zero effort" is the winning copy pattern. Before/after visuals of a raw snip → beautified shot. Speed claim "beautiful screenshot in 2 seconds" | Emoji-heavy whimsical tone clashes with engineering aesthetic; watermark-as-paywall isn't our model |
| **Screen Studio** — screen.studio | Cinematic premium. Black bg, white type, Manrope, one white CTA pill; motion everywhere | **Video-led**: every feature is a self-playing demo clip with **in-feature tab switchers** (Zoom \| Editing \| Clarity); massive "Used by people at" logo wall (~150 companies); "Made with Screen Studio" UGC video wall (Stripe, Notion, levelsio…) | The per-feature tabbed video module — the *exact* pattern Quickshort's showcase section already uses; take it further: each feature cell could be a looping clip. Auto-zoom + cursor smoothing + click-ripple grammar for our Remotion video. UGC wall reframes proof as output | Needs an enormous volume of polished footage; a half-empty "made with" wall backfires |
| **Raycast** — raycast.com | The engineering-aesthetic north star, evolved past the cliché: dark, glassy, hardware-precise | **Interactive hero** (a real keyboard you can press); values stated as specs ("Fast. Think in milliseconds. Ergonomic. Keyboard first."); extension cards grid; testimonials as interactive cards with "favorite feature" metadata | **Values-as-specs block** — "Fast. Ergonomic. Reliable. 99.8% crash-free" reads like a spec sheet, not marketing. Interactive product artifact in hero (for us: a working selection-rect drag, or pressable ⌘⇧S keycaps). Keyboard-first identity = screenshot tools' exact audience | Building a hardware keyboard hero is a budget we don't have; adapt the *idea* (real product artifact in hero), not the artifact |
| **Linear** — linear.app | Muted, disciplined, blueprint-like ("Fig 0.1 / 0.2" figure captions) | Product UI *is* the hero — a live-looking Linear issue rendered above the fold; sections are labeled figures; real product surfaces as every illustration | **Fig. labels** (`Fig 0.1`) — cheap, distinctive engineering-paper motif Quickshort can adopt per screenshot. Real-UI-as-illustration instead of abstract art | Linear's scale lets real UI carry the page; needs our product shots to be genuinely beautiful first |

---

## 3. Pattern analysis — what's recurring, what's saturated

Recurring across the set (fetched pages + the 100-site dev-tool study by Evil Martians):

| Pattern | Who uses it | Verdict |
|---|---|---|
| Centered hero, bold headline, product visual below | Nearly universal among dev tools (Evil Martians: "the vast majority") | **Baseline, do it.** Quickshort complies. Differentiation must come from the visual *inside* the frame |
| Trust block right after hero | CleanShot (250k users + logo wall), Screen Studio (~150 logos), Kap (community logos), ShareX (33M downloads) | **Do it.** Quickshort currently has no post-hero trust strip — GitHub stars, download counts, "free · open source · no account" stats fit perfectly in the existing marquee strip slot |
| Bento / mixed-size feature cards | Linear, Raycast, Vercel, Supabase per multiple sources; CleanShot uses mockup-led sections instead | **Saturated but still the correct density answer.** Our 12-cell uniform grid is a bento that forgot to vary — introduce 2× cells for hero features (annotate, recording) |
| Hairline 1px borders over shadows | Vercel tokens (#2E2E2E edges), designmd ("borders over shadows… never drop shadows"), our `--line:#262626` | **Canonical.** Keep |
| "+" cross marks at grid intersections | Vercel signature; copied by every Vercel-style template; our landing + ui-2026 | **Approaching cliché** — keep but reduce frequency; only on true grid junctions, not every frame corner |
| Mono eyebrow labels + letterspaced uppercase | Ours (`eyebrow`, `sec-label`), Vercel-style templates everywhere, Screen Studio captions | **Canonical.** Keep — it's the cheapest genre signal |
| Tab-switched product showcase | Screen Studio (per-feature tabs), Quickshort showcase (radio-label tabs), CleanShot feature tabs | **Strong pattern.** Ours is already implemented; extend tabs into feature cells |
| Numbered pipeline / numbered loop | Zappy (4 steps), our pipeline section, Linear cycles | **Good fit** for "capture → annotate → act" story |
| Keycaps / keyboard hero artifacts | Raycast (whole keyboard), ui-2026 (⌘⇧S caps), Screen Studio keystroke overlays | **High-value, low-cost.** Keycaps are our cheapest authentic artifact — the product is literally a hotkey |
| "Vs built-in tool" comparison table | Snagit's entire second act | **Opportunity** — nobody in the OSS tier does this well. Quickshort vs macOS shot tool vs Snipping Tool |
| Animated dash-lines / grid glow | Ours (`runv`/`runh`), every dark dev-tool clone | **Cliché risk** — subtle enough today; don't add more glow/aurora |
| Testimonial wall / UGC wall | CleanShot (350+ tweets, per-section placement), Screen Studio ("Made with" video wall), Xnapper (wall of love), Kap | **Partially applicable** — Quickshort's sponsor wall + GitHub issue quotes could seed this, but don't fake scale |
| Emoji-in-copy playful tone | Xnapper, ShareX feature lists | **Avoid** — off-brand for the engineering register |

---

## 4. Design-language proposal for Quickshort

### 4.0 Tokens (evolve ui-2026, don't replace)

```css
--bg:        #000;              /* keep — pure black is ours; Vercel uses #0A0A0A */
--bg-raised: #0a0a0a;
--ink:       #ededed;
--muted:     #a1a1a1;
--faint:     #6b6b6b;
--line:      #262626;
--line-soft: #1a1a1a;
--accent:    #52c7ea;           /* interactive/active ONLY: focus, active tool, pressed key */
--capture:   #52c7ea;           /* selection rect + handles — same hue, dashed 1.5px (ui-2026 .block) */
--grn:       #30d158;           /* success only (toast ✓, "saved") */
--red:       #ff5f57;           /* error only */
--radius:    8px;               /* cards/frames; 6px inner; 999px pills */
--sans:      "Geist";           /* headings -0.04→-0.06em at display sizes (matches Vercel tracking ladder) */
--mono:      "Geist Mono";      /* labels 10–12.5px, tracking .16–.24em, uppercase */
```

**Spacing philosophy:** borders define surfaces, not shadows (drop shadows only inside *product imagery*, never on UI chrome). Cells separated by 1px shared borders (`gap:1px` trick already in use). Section padding 110px desktop — keep.

**Signature motifs to own** (differentiators *inside* the genre):
- **Selection rect**: 1.5px dashed accent rect + small square corner handles (already in ui-2026 `.block`) — use it in the logo lockup, hero background, loading states, OG image. This is our equivalent of Raycast's window or Vercel's triangle.
- **`W×H` size counter**: mono `1280 × 720` readout pinned to a selection's corner — reuse in graphics, OG images, the favicon-adjacent brand marks. Nobody else owns this.
- **Fig. captions**: label product shots `FIG 01 — OVERLAY` in mono, Linear-style.
- **Keycap chips**: `⌘ ⇧ S` as recurring inline artifact (ui-2026 `.key` / `.kbd-inline`).

### 4.1 Landing page

Current page is structurally sound (hero → strip → showcase → features → pipeline → download → request → donate). Targeted upgrades:

1. **Hero**: add a measurable spec to the sub or as a mono stat row — e.g. `NATIVE RUST · 0 ELECTRON · @2X PIXELS ALWAYS`. Optional power-move (from Evil Martians/Raycast): a **live mini-artifact** under the CTAs — a draggable selection rect over a dimmed screenshot, or pressable `⌘⇧S` keycaps. Highest differentiation per dollar.
2. **Strip**: convert the marquee into a trust/spec strip — add `★ GitHub stars`, `FREE FOREVER`, `OPEN SOURCE`, `NO ACCOUNT`. ShareX proves stats work; we just don't have logos yet.
3. **Showcase**: already correct pattern (Screen Studio-style tabs). Add `FIG` captions + keep video first.
4. **Features**: break the uniform 12-cell grid — make Annotate and Screen Recording **2× hero cells containing media** (the `.cell-media` style already exists in the CSS, unused). Move remaining features into tighter cells. Every claim should carry a number where possible.
5. **New section: "vs the built-ins"** — Snagit's comparison-table idea, honest version: Quickshort vs macOS Screenshot vs Snipping Tool on rows like Retina output, scrolling capture, editable annotations, share links, recording, price. A free tool wins this table outright.
6. **Donate**: keep current structure (GH Sponsors + VietQR is genuinely distinctive); reframe headline toward OSS-funding honesty ("free forever · funded by a few" already good). Steal Kap's contributor/community framing for the wall.
7. **Motion**: keep reveal-on-scroll only; the running dash-lines are the max allowed atmosphere. No aurora gradients, no parallax.

### 4.2 In-app UI (overlay / toolbar / toasts)

Conventions found: tools on single keys (Snapok `R O A P M T V`; Shottr 16-tool toolbar); size counter attached to selection (Firefox Screenshots `{w} × {h}`, Shottr's clickable dimension box); toolbars attached to the selection without overlapping (macshot notes); a floating post-capture card rather than a bare toast (CleanShot Quick Access Overlay — "annotate/copy/drag right there"); dismissible-by-drag notifications (Shottr).

Proposed styling, all tokens already exist in ui-2026:

- **Dim**: black at ~40–45% (macshot uses `0.4`) — deep enough to read the selection, light enough to see context.
- **Selection**: 1.5px dashed `--accent` rect + 11px square handles at corners/midpoints (`.block` spec); `W × H` mono counter outside top-left of the rect, `--muted` on `--bg-raised` chip.
- **Toolbar**: floating hairline-bordered bar (`--bg-raised`, `--line`, 8px radius) attached below-selection (flip above when clipped). Tools as mono glyphs in 36px squares — matches `.cell-glyph`. Active tool = accent text + `rgba(82,199,234,.1)` fill (`.chip.acc` spec). Color swatches as a horizontal strip *inside* the bar; size stepper mono `1 2 3`.
- **Toasts** → graduate to **action cards**: keep ui-2026 `.toast` (icon cell + title + mono sub) but attach 3–4 ghost mini-buttons (Save · Copy · Pin · Link) — CleanShot's "one more thing hovering" critique is answered by our auto-dismiss + drag-out dismissal (Shottr's pattern).
- **Text/labels**: Geist for UI, Geist Mono for readouts. Letterspaced uppercase mono only for section labels, never for buttons (buttons stay sentence case or mono lowercase commands like `brew install…`).
- **Settings window**: ui-2026 `.win` spec is already correct — traffic-light row, hairline rows, `.kbd-inline` hotkey chips, status `.chip ok`. Ship it as-is.

### 4.3 Demo video (Remotion art direction)

Steal Screen Studio's grammar, render it in our tokens:

- **Frame**: pure black stage, dashed hairline grid faintly visible (landing's `vline`/`hline`), "+" marks at frame corners for title cards only.
- **Structure**: hook (3s: hotkey → all monitors dim) → per-feature segment (8–12s each) → pipeline/diagram beat → OSS/free card → download end-card. Screen Studio's proof: one idea per clip, tabs/labels naming it.
- **Captions**: ui-2026 `.caption` pill — mono uppercase, dot accent — bottom-center, one per beat ("ONE HOTKEY. EVERY MONITOR DIMS." already written).
- **Cursor**: smooth, slightly oversized; **click ripple** + optional subtle motion blur (Screen Studio's signature feel); hide when idle.
- **Zooms**: spring-eased zoom into the selection/toolbar on each beat — our cubic-bezier `(.16,1,.3,1)` everywhere, matching landing reveals.
- **Keycaps**: `⌘⇧S` keycap group animating in (staggered rise) whenever a hotkey is "pressed" — this is the video's recurring motif.
- **Toasts**: reuse `.toast` component verbatim for save/copy/share beats; the green ✓ only for "saved."
- **End card**: headline "Select. Snip. Ship." + `brew install --cask quickshort` mono pill — same two CTAs as the landing.
- **Avoid**: voiceover-less walls of text, bouncing-emoji energy (Xnapper), stock swooshes. The video should look like the landing page learned to move.

---

## 5. Sources

Pages fetched (first-party):
- https://cleanshot.com — hero, testimonials, logo wall, features, cloud, studio mode
- https://cleanshot.com/pricing (via search snippet) — $35 Basic one-time / $10·user·mo Pro; "Pay once, use forever"; cleanshot.com/why-updates-expire — $19/yr optional renewal
- https://shottr.cc — rotating-audience hero, "2.3mb dmg / 17ms" specs, feature list, FAQ, changelog (16-toolbar-tools, drag-to-dismiss notification, menubar confirmation animation)
- https://snagit.webflow.io — TechSmith's current marketing lander ("still using snipping tool", vs-built-ins table, $39/yr, Fortune-500 claim)
- https://monosnap.com — JS-gated shell (nav only: Download / Pricing / Sign In / Enterprise); plans via monosnap.zendesk.com plan comparison
- https://zapier.com/zappy — 4-step numbered flow, free, cloud behind Zapier account
- https://flameshot.org — feature blocks, per-OS download matrix with package-manager commands, sponsor logos, GPLv3/OSI badges
- https://getsharex.com — stats ("33M downloads, 18 years, privacy first"), capture/after-capture/upload/tasks taxonomy
- https://getkap.co — minimal page, window mockup, plugin cards, community logo wall
- https://xnapper.com — benefit-led hero, auto-everything features, 4.9★/4000-creators proof, awards laurels, free + watermark-removal pricing; changelog for mesh-gradient/background names
- https://screen.studio — video-led features with tab switchers, ~150-company logo list, "Made with Screen Studio" UGC wall, layouts/zoom/cursor-motion features
- https://raycast.com — pressable-keyboard hero, "Fast/Ergonomic/Personal/Reliable" spec-values, extension cards, interactive testimonial cards
- https://linear.app — live-product hero, "Fig 0.x" captions, muted palette, real-UI illustrations

Search results / third-party analyses cited:
- https://evilmartians.com/chronicles/we-studied-100-devtool-landing-pages-here-is-what-actually-works-in-2025 — 100-site study: centered heroes, trust blocks post-hero, numbers-not-logos for individual tools, dual CTAs
- https://designmd.app/library/bento-grid-tech-minimalist — bento lineage (Apple → Linear → Vercel → Raycast), "borders over shadows, 1px at 8–12% opacity"
- https://rationalgo.ai/resources/app-builder/prompts/vercel-style-dark-website-prompt — measured Vercel tokens (#0A0A0A/#EDEDED/#A1A1A1/#2E2E2E, -0.06em tracking, plus-crosses) — proof the look is now a mass-produced template genre
- https://www.saasdesign.io/prompts/devtools-landing-page/ — "Vercel-style" shipped as a starter template = saturation evidence
- https://saaslandingpageexamples.com/examples/cleanshot-x — CleanShot pattern tags (bento, sticky CTA, testimonial)
- https://refto.one/cleanshot/home/2026-01-26 — CleanShot visual analysis (white space, black headlines, single blue CTA, "when everything turns blue the system loses clarity")
- https://onepagelove.com/cleanshot-x and https://onepagelove.com/screen-studio — typefaces (Inter; Manrope) and section inventories
- https://designmunk.com/screen/ — Screen Studio tokens (black bg, white type, Manrope, hairlines)
- https://typenorm.com/apps/cleanshot — Quick Access Overlay UX description + "one more thing hovering" critique
- https://cleanshot.com/features (snippet) — Quick Access Overlay feature list (drag-drop, auto-close, gestures)
- https://www.macworld.com/article/617854/cleanshot-x-review.html — floating thumbnail "command center" description; editable annotations vs macOS Markup
- https://www.bitdoze.com/shottr-mac-screenshot-tool/ and https://www.howtogeek.com/reasons-i-use-shottr-instead-of-the-mac-screenshot-tool/ — Shottr toolbar/editor/shortcut inventory
- https://snapok.app/features/screenshot-annotation — one-key-per-tool mapping (R O A P M T V, sizes 1/2/3)
- https://croc100.github.io/Reticle/docs/annotate/ — toolbar-at-top-of-overlay, swatch strip, 1–12px stepper, sticky mode
- https://github.com/duongductrong/Snapzy (RecordingRegionOverlayWindow.swift) — overlay constants: 40% dim, 1.5px border, handle geometry
- https://github.com/sw33tLie/macshot/releases/tag/v4.2.0-beta.1 — selection-attached toolbars, split W/H size fields
- https://github.com/mozilla-firefox/firefox commit 8331e6f — Firefox Screenshots `{w} × {h}` overlay counter
- https://monosnap.zendesk.com — plan tiers and dropzone UX
- https://thegtmdirectory.com/tools/cleanshot-x and https://www.webfolio.to/tools/cleanshot-x — CleanShot $29 pricing context vs Snagit/Loom

Local files read (current design baseline):
- `docs/index.html`, `docs/style.css` — tokens, grid-cells, showcase tabs, marquee strip
- `demo/ui-2026.html` — video/UI kit: toasts, keycaps, captions, window chrome, dashed selection block
