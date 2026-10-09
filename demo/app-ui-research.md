# Quickshort — In-App UI Research

Component-level design reference for every Quickshort surface, based on teardowns of
CleanShot X, Shottr, macOS Screenshot, Snagit, Flameshot, ShareX, Xnapper, Screen
Studio, KDE Spectacle, Windows Snipping Tool, GNOME Screenshot, and OSS clones —
mapped onto the `ui-2026.html` token system. Companion to `design-research.md`
(which covers the landing page); this file is **in-app only**.

Code references are to `crates/` at time of writing:
`qs-overlay/src/lib.rs` (overlay), `qs-overlay/src/toolbar.rs` (toolbar),
`qs-overlay/src/annotate.rs` (palette/text/blocks), `qs-app/src/quick_access.rs`,
`qs-app/src/toast.rs`, `qs-app/src/recstop.rs`, `qs-app/src/pin.rs`,
`qs-app/src/settings.rs`, `qs-app/src/tray.rs`, `qs-app/src/widgets.rs`,
`qs-app/src/config.rs`.

---

## 1. TL;DR — design decisions

1. **Reskin everything to the dark ui-2026 tokens — one theme, no light mode v1.**
   Today's in-app chrome is split: toolbar discs are dark (`#2C2C2E`), but the toast,
   quick-access card and settings window are **light macOS-style** (`#FFF` card,
   `#CBD0D8` border). Move all floating chrome to `--bg-raised:#0a0a0a` +
   `--line:#262626` hairline + `--accent:#52c7ea`. The landing and the app then read
   as one product; dark chrome also photographs correctly over both bright and dark
   screen content (Flameshot/Spectacle/CleanShot all run dark overlay chrome).
2. **The selection rect is the brand.** Steal from the ui-2026 `.block` spec that
   already exists: dashed 1.5px accent border + 11px square corner/mid-edge handles
   on `#000` fill. Upgrade the current solid `frame_rect` (lib.rs:941) to dashed
   while *dragging* and show the 8 handles once the selection is *confirmed*
   (annotation phase). Nobody else owns a dashed-selection-rect mark — it doubles
   as landing imagery.
3. **Add the two things the overlay is missing: crosshair + `W×H` counter.** Code
   today has neither (verified: no text render in `qs-overlay` outside annotations;
   no crosshair draw). Every serious competitor has both: Shottr's clickable
   dimension box (logical⇄physical px on click — steal this verbatim, it's free
   Retina bragging), Firefox Screenshots' `{w}×{h}`, Flameshot's `xywh` tick,
   GNOME's crosshair pointer. Spec: full-width 1px hairlines at ~12% white through
   the cursor + a mono chip `1280 × 720` pinned above the cursor/selection corner.
4. **Toolbar: keep the geometry (bottom-dock → flip cascade + 28px discs), change
   the skin and add grouping.** The docking cascade bottom→top→right→left→inside
   (toolbar.rs:172-206) is already best-in-class (macshot/Screenshotli attach the
   same way). Keep 28px circular buttons; add 2px hairline separators between
   `tools | history | actions`; recolor discs `#0a0a0a`/`#262626`, active = accent
   disc with black glyph. The current mixed tool+action row needs the separation —
   CleanShot keeps Annotate tools in the editor, terminal actions in the QAO;
   Snipping Tool's "Quick markup" (in-overlay markup before commit) validates our
   single-bar fusion, but visually the two groups must read differently.
5. **One key per tool, mono hint on hover.** CleanShot's annotate keymap is the
   best documented (`V D L A R F E C T M H P K B`), Shottr's is single-key
   (`a` arrow, `t` text, `b` blur). Arm Quickshort tools the same way — map to
   what we ship: `V` select, `P` pen, `A` arrow, `R` rect, `O` ellipse (ovals),
   `M` marker, `T` text, `X` pixelate — and surface the key in a tooltip chip,
   not on the button (keeps the bar clean like Shottr's).
6. **Replace the quick-access card with a passive capture stack.** The toolbar
   already committed the action — a post-capture decision card is redundant
   (and typenorm's "one more thing hovering" critique applies to it too).
   Instead: the captured region animates **flying to the `popup_pos` corner**
   and lands on a persistent album stack — macOS's floating-thumbnail flight,
   extended into a shelf. Newest on top, hover fans the stack out, each thumb
   is a drag source + click-to-open + right-click actions menu (Copy / Reveal /
   Share / Beautify / Pin / Delete). Uploads show a ◌ badge that flips to ⧉ on
   success, ✕ on failure. No pills, no countdown, no card. (Direction confirmed
   by maintainer — supersedes the earlier reskin-the-card proposal.)
7. **Toasts graduate to ui-2026's icon-cell layout.** Current toast draws a filled
   colored circle glyph; ui-2026 spec is a 30px **bordered square cell** with
   colored glyph + title + mono sub-line + 2px countdown. Keep 272→~300px width,
   58→64px height. Drag-up-to-dismiss (Shottr's notification) is the one
   affordance worth adding.
8. **REC dock: stay minimal — dot + `REC m:ss` + Stop — but match Screen Studio's
   semantics.** Screen Studio's recording widget does finish/pause/restart/delete
   and hides via right-click. We only need Stop today; spec the card in tokens
   (`#0a0a0a`, pulsing `#ff5f57` dot, mono timer, red Stop pill on hover) and
   leave a slot for a pause pill if recording gains it. GNOME's top-bar red dot
   confirms "one persistent red indicator per screen" is enough.
9. **Settings: keep macOS-System-Settings layout, reskin dark.** 820×600, 200px
   sidebar, 52px rows are all correct; swap the light palette (`#FFF`/`#F8F9FB`/
   `#E1E4E9`) for `#0a0a0a`/`#050505`/`#262626` with `.kbd-inline` keycaps and
   accent chips. Raycast's v2 settings redesign and macOS System Settings both say:
   grouped inset cards beat flat lists.
10. **Cross-platform: tokens stay, platform chrome adapts.** Keep `#0a0a0a`/hairline
    design on Windows/Linux (Flameshot/Spectacle/ShareX all ship their own chrome
    everywhere); adapt key glyphs (⌘→Ctrl, ⌥→Alt, return label), corner radius
    (Windows wants squarer — keep 8px, it's mid-way), and tray/menu conventions
    (native menus per OS — never fake a context menu).

---

## 2. Component inventory

Legend: **exists** = rendered today · **partial** = exists but underspecified/wrong theme ·
**needs-design** = absent in code. "ui-2026" tokens: `--bg #000 · --raised #0a0a0a ·
--ink #ededed · --muted #a1a1a1 · --faint #6b6b6b · --line #262626 · --soft #1a1a1a ·
--accent #52c7ea · --grn #30d158 · --red #ff5f57 · r 8px (cards) / 6px (inner) / 999px (pills)`.

| # | Surface | State today | Competitor reference | Proposed spec (ui-2026 tokens) |
|---|---|---|---|---|
| 1 | Dim layer | exists — `(p>>1)` 50% darken per px (lib.rs:292) | macshot/Snapzy 40%; Flameshot configurable backdrop opacity; CleanShot dims except record region | Keep 50% `#000`. While recording region armed: keep dim outside region. Opacity goes to settings later (Flameshot exposes it) |
| 2 | Crosshair | **needs-design** — none | Shottr true-zoom lens; CleanShot ⌘-crosshair+magnifier; GNOME crosshair pointer; Flameshot magnifier widget | 1px `#ededed` @ ~14% alpha, full monitor span, 4px gap around cursor point. Phase 2: 96px magnifier circle (Shottr) |
| 3 | Selection rect (dragging) | exists — undim + **solid** 1.5px accent `frame_rect` | ui-2026 `.block` dashed; Snapping Tool grabbers; Snapok dashed | Dashed 1.5px `--accent` (8px dash / 4px gap) while dragging; interior un-dimmed |
| 4 | Selection handles (confirmed) | **needs-design** | Snipping Tool "grabbers around the perimeter"; GNOME corner handles; ui-2026 `.block i` | 11px squares, `#000` fill + 1.5px accent border, at 4 corners + 4 mid-edges; hover → accent fill |
| 5 | `W×H` size counter | **needs-design** | Shottr clickable dimension box (logical⇄physical); Firefox `{w}×{h}`; Capso dimension display | Mono 11px chip: `1280 × 720` on `#0a0a0a`+`#262626`, r6, above top-left of selection (below if clipped); click toggles logical/physical (Shottr parity — free on `cscale`) |
| 6 | Annotation toolbar | exists — 28px discs `#2C2C2E`, cascade dock bottom→top→right→left→inside | CleanShot (tool-per-key), Shottr (16 tools), Flameshot (toolbar + Space side panel), Snipping Tool Quick markup (in-overlay) | Keep cascade + discs. Skin: `#0a0a0a` disc, 1px `#262626` rim (replaces 12% white rim), hover `#1a1a1a`, active = accent disc + `#000` glyph, disabled 35%. Separators between groups |
| 7 | Tool icons | exists — SDF 1.8px strokes | SF Symbols-like thin stroke convention (Raycast/CleanShot) | Keep SDF glyphs; ensure optically centered; hover raises to `--ink` 100% |
| 8 | Color swatch strip | exists — 8 discs r6 in options row | Flameshot radial picker (right-click); Xnapper swatch+blue-ring selection | 8 discs from `PALETTE`; selected = accent ring (1.5px) outside disc, not fill (works on white/black swatches) |
| 9 | Stroke-size picker | exists — 3 filled discs r 1.8/3.0/4.6 | Snagit quick styles; Snapok `1 2 3`; Flameshot wheel-scroll thickness | Keep 3-dot discs; add mouse-wheel thickness while armed (Flameshot's best trick — zero UI) |
| 10 | Text tool editor | exists — click-place, caret, drag-select, IME preedit via `set_ime_cursor_area` | Shottr text w/ pointy arrow; ShareX speech balloons | Caret `--accent` 1.6px (currently text color — change: caret should signal editing, not output color); selection = accent @35% (already); font-size pills `12 14 18 24 32 48` + `B` in options row (exists — keep) |
| 11 | Undo / Redo | exists — dimmed at 35% when empty | Every editor | Keep; hover tooltip `⌘Z` / `⌘⇧Z` |
| 12 | Editable annotation blocks | exists — select/move/delete, 1.2px accent bbox outline | CleanShot "everything stays editable" | Selected shape: dashed accent bbox + 4 corner handles (same 11px spec as selection) — distinguishes *editing* from *region* state |
| 13 | ⌘-action hint chips | **needs-design** (keys exist in config, not surfaced) | macOS ⇧⌘5 Options; CleanShot hover shortcuts | Under each action button on hover, 11px mono chip `⌘S`/`⌘C`/`⌘T`/`⌘L`/`⌘G`/`⌘R` — or a persistent second label row; cheap: tooltip chip |
| 14 | Capture stack (post-capture) | exists as quick-access card — 424×108, **light**, 5 pills, 8s TTL | macOS floating thumbnail (flight), CleanShot QAO (drag-thumb, stacking), DodoShot (hover-pause) | **Replace card with passive album stack**: shot flies to `popup_pos` corner, newest on top, ~5 visible + count chip; hover fans out; thumb = drag source, click→open, right-click→actions; badge = ✓ saved / ⧉ link / ◌ uploading / ✕ failed-retry; lingers until used, × deletes |
| 15 | Toasts | exists — 272×58, **light**, filled-circle icon | ui-2026 `.toast` spec; Shottr drag-up-to-dismiss | Icon = 30px square cell w/ tinted border (`.ic ok/info/err/work`), title 14px/550, mono sub 11px `--muted`, 2px countdown accent strip, TTL 1.8s, drag-away dismiss |
| 16 | REC dock | exists — 172×46 dark card (already dark!) | Screen Studio control widget (finish/pause/restart/delete, right-click hide); GNOME top-right red dot | Reskin to tokens: `#0a0a0a`, pulsing `#ff5f57` dot r5 @~1.5Hz, `REC 0:07` mono 12, Stop pill 52×26 → `--red` on hover; same `popup_pos` docking |
| 17 | Pin window | exists — borderless ≤560×400, 0.6px border, × circle r9 | Shottr scroll-resize pins, semi-transparent no-shadow pins; StickyShot scroll-opacity | Border → 1px `#262626`; × circle → `#0a0a0a`/`#262626`, hover `--red`; add scroll = resize (Shottr), ⌥scroll = opacity later |
| 18 | Settings window | exists — 820×600 sidebar, **light** macOS style | Raycast v2 settings; macOS System Settings; HIG grouped lists | Reskin dark: sidebar `#050505`, content `#000`, cards `#0a0a0a`/`#262626`, rows 52px keep, keycap record chips `.kbd-inline` style, switches accent-filled when on |
| 19 | Tray / menu bar menu | exists — muda native menu | HIG menu-bar extras; CleanShot/Shottr menu UX | Native per-OS menu (correct today); item order Capture → Recents → sep → Open Folder → Settings → Launch at Login → sep → Update → Support → Quit; template-style monochrome icon (exists — SDF crop glyph) |
| 20 | Update affordances | exists — tray item + About row | CleanShot swaps-in-place; Raycast menu-bar check | Tray `Update to vX — Restart to Install` (exists); About row chip `V0.2.1 ✓`/`UPDATE →` (`.chip.ok`/`.chip.acc`); optional accent toast on background-detected update |
| 21 | Overlay action strip (pre-selection help) | **needs-design** | Flameshot on-screen shortcut hints; GNOME mode buttons | Optional: first-run hint pill bottom-center `DRAG TO SELECT · ESC TO QUIT` (`.caption` spec), fades after 3 uses — low priority |
| 22 | Countdown / scrolling-capture state | partial — scroll capture exists, no dedicated chrome | CleanShot timer/scroll affordances | Reuse REC dock styling with accent dot + `SCROLL` label when that mode is active |

---

## 3. Competitor teardowns (in-app UI only)

### 3.1 CleanShot X — the reference

**Quick Access Overlay (the "command center").** After every capture a small
floating thumbnail appears docked in a screen corner (default bottom-left).
Copy and Save buttons are overlaid directly on the thumbnail corners; the whole
thumbnail is a live drag source into any app/Finder. Click → Annotate window;
right-click → context menu (incl. "Move to trash", added 4.1 which also moved the
close button and added QAO keyboard shortcuts). Multiple captures stack as
multiple overlays; position, size, auto-close and persistence are all user
configurable; swipe gestures control it; a "restore recently closed overlay"
command exists. [cleanshot.com/features, macworld review, imore 4.1 notes]

**Annotate window.** A dedicated editor window (not in-overlay). Tool-per-key
bindings, best documented at defkey/alchemists: `V` move · `K` crop · `D` draw ·
`M` highlighter · `L` line · `T` text · `A` arrow · `C` counter · `E` ellipse ·
`P` redaction/pixelate · `H` spotlight · `R` rect · `F` filled rect · `B`
background. `⌘S` save, `⌘⇧C` copy image, `⌘U` upload, `⌘W` cancel. Signature:
the **counter tool** drops auto-incrementing numbered circles — every review
calls it out. Everything stays editable; CleanShot saves editable project files.
[defkey.com/cleanshot-x-shortcuts, alchemists, macworld]

**Capture overlay.** Area capture with optional crosshair + **magnifier**
(advanced prefs; ⌘ shows crosshairs while dragging); "freeze screen" mode; an
All-In-One HUD that puts every capture mode on one floating toolbar with
aspect-ratio lock and remembered last selection. [cleanshot.com/features,
alchemists]

**Recording.** Desktop dims except the recorded region (dimming can be disabled
— then the region gets a red border); recording time displayed **in the menu
bar**; options for cursor visibility, click highlights, keystroke display,
camera bubble, mic/computer audio, auto-DND, hide desktop icons.
[cleanshot.com/features, alchemists]

**Steal:** QAO = drag source + click-to-annotate + configurable lifetime;
counter tool (auto-increment badges — our text tool could do `1 2 3` chips);
everything-editable promise; menu-bar recording timer as an alternative to a dock.
**Avoid:** the "one more thing hovering demanding a decision" critique
(typenorm) — answer with honest auto-close + hover-pauses + drag-away.

### 3.2 Shottr — the precision editor

Editor window: top toolbar holds **all 16 tools when the window is wide
enough**; a **properties panel top-right** with the current tool's options plus
a **clickable dimension box** that toggles logical vs physical (Retina) pixel
readout. Ruler tool: `1`/`2` for vertical/horizontal measurement, `Shift` for
outer size, click imprints the measurement onto the image. Smart Select: ⌥ while
selecting auto-balances padding. Selection nudging: arrows move 1px, `⌘`+arrows
resize, `Shift` = 10px, `[ ]` resize symmetric. `Q`/`W` zoom to selection
corners; `Z`+drag zooms into a region; `Tab` copies the color under the cursor
(hex/OKLCH/APCA). Custom notification **dismissed by dragging it up**; menubar
icon plays a confirmation animation on copy/save/upload. Pins: scroll to
resize, semi-transparent pins lose their shadow. Editor opens on the screen the
capture was taken on. [shottr.cc, bitdoze review, howtogeek, screensnap.pro,
buttondown v1.5.3]

**Steal:** the dimension box (we have `cscale` — it's nearly free);
arrow/⌘-arrow/Shift-10px nudging verbatim; drag-away dismiss; "opens on the
capture's screen" (our popups are currently stuck on the primary monitor —
widgets.rs `popup_monitor` — fix that). **Avoid:** the dense 16-tool wall is
fine for a $8 pixel-tool; our bar should stay ≤16 items but visually grouped.

### 3.3 macOS built-in Screenshot (⇧⌘5)

A single **bottom-center floating bar** (draggable, dismiss with ✕ or Esc):
capture screen / window / portion | record screen / portion | **Options**
pop-up (Save To, Timer, Show Floating Thumbnail, Remember Last Selection, Show
Mouse Pointer) | Capture button (Enter triggers). After capture: a **floating
thumbnail bottom-right for ~5s** — drag into a document, or click to open the
Markup window (sketch/shapes/text/signature/crop/rotate + Share/Trash/Done).
Region select uses a crosshair pointer; Space switches to window mode with a
camera icon. [Apple support mh26782, macmost]

**Steal:** the Options pop-up pattern (a single ⚙ on the toolbar could hold
timer/pointer/remember-selection later); thumbnail→editor flow is exactly our
quick-access card. **Avoid:** 5s vanishing thumbnail is the #1 user complaint —
our 8s card with hover-pause and "keep until used" option is already better.

### 3.4 Snagit editor

Editor: **customizable top toolbar** (drag tools in/out/reorder, group with
spaces, icon-only mode, share destinations can live in the bar). Selecting a
tool opens its **Quick Styles panel** — preset combos auto-generated from the
active **Theme** (palette + fonts + shadow on/off); user-saved Quick Styles and
Favorites (yellow star). Right side: properties + **Effects panel** (borders,
torn edges). Bottom: **Recent Captures Tray** — thumbnail strip, pin-able to the
left, right-click batch ops. Share = dropdown of destinations.
[techsmith tutorials ×4]

**Steal:** Quick Styles as *saved swatch+size presets* — our options row could
grow a "save current style" `+` disc cheaply; the recents tray concept maps to
our tray-menu recents. **Avoid:** toolbar customization UI — enterprise weight
we don't need.

### 3.5 Flameshot — the keyboard-native overlay

Overlay: dimmed screen, selection with **handles** (`Ctrl`+drag = keep aspect,
`Shift`+drag = mirror-resize), live **x/y/w/h readout** (`xywhTick`), optional
magnifier. Tool buttons dock **along the selection edge**; **right-click opens a
radial color palette** at the cursor; **mouse wheel changes tool thickness**;
**Space opens a side panel**: tool-size spinbox+slider (1..N px), active-color
label, full **color wheel + hex input**, and an **undo-history list** where
clicking an entry reverts to it. On-screen **help/hint message** lists bindings.
Tool keys: `p d a s r c m t b g`. Settings let you theme the UI color, contrast
color, and backdrop opacity, and turn off buttons you never use.
[flameshot.org key-bindings, howtogeek, sidepanelwidget.cpp, capturewidget.h]

**Steal:** wheel-to-thicken (zero chrome, huge win), right-click radial color
picker as a *second* color UI behind our swatch strip, Space=options panel as an
escape hatch if the options row ever overflows, aspect/mirror resize modifiers.
**Avoid:** the side panel itself — it's a Qt widget tree; our options row covers
the 95% case.

### 3.6 ShareX

`RegionCaptureForm` runs modes: Default → Annotation → Editor → TaskEditor.
Shape taxonomy is instructive: **Region** (rect/ellipse/freehand — defines what
to keep), **Drawing** (rect/ellipse/line/arrow/text/outline-text/speech
balloons/step numbers), **Effect** (blur/pixelate/highlight/magnify/spotlight),
**Tool** (select/crop/cut-out/smart-eraser). The image editor's **bottom
toolbar** carries the terminal actions: continue-task / cancel / copy / save /
save-as / pin-to-screen / print / upload. After-capture tasks are a named,
configurable workflow stage (quick task menu exists too). New Avalonia editor
keeps the same host callbacks (copy/save/save-as/upload/pin).
[getsharex.com image-editor docs, ShareX.ImageEditor repo, deepwiki]

**Steal:** the Region/Drawing/Effect/Tool taxonomy is exactly our
tools-vs-actions split — use it as the grouping logic for toolbar separators.
**Avoid:** modes-within-modes; our single fused overlay is simpler and right.

### 3.7 Xnapper — the beautifier

Editor opens with the shot **already beautified** (auto-balance, auto
background, rounded corners, shadow). Right-side control column: **Background**
picker (colors/gradients/uploaded images, named gradients "Flower"/"Beach"),
**Padding / Inset / Shadow / Rounded** sliders whose values are **clickable
numbers** for exact entry, **Redact** (one-click ML detection of
emails/CC/IPs/API keys + manual pixelate; transparent-vs-solid styles), **ratio
presets** (16:9, 4:3, social sizes). Selected options show a blue highlight
ring. Has a literal "**Drag me**" button. [xnapper.com, blog/annotate,
changelog, App Store notes]

**Steal:** our Beautify pill is one-shot today — Xnapper proves the demand is a
*panel*: on Beautify click, swap card buttons for `Padding − · +` / `Radius` /
background swatches with click-to-type values. Auto-redact is beyond scope but
manual pixelate already exists. **Avoid:** emoji whimsy; keep it engineering.

### 3.8 Recording-state UI (Screen Studio · CleanShot · GNOME)

- **Screen Studio:** a small persistent **recording control widget** — elapsed
  timer + finish / pause / restart / delete; right-click → "Hide current
  recording control"; also menu-bar stop, dock right-click → Finish, global
  stop hotkey. [screen.studio guides]
- **CleanShot:** dims everything except the record region (red border when
  dimming off); elapsed time in the **menu bar**. [features, alchemists]
- **GNOME:** a **red dot in the top-right corner of the screen** showing elapsed
  seconds; click it to stop. The recorded area also gets a persistent outline
  indicator. [help.gnome.org, gnome-shell commits]

**Consensus spec for Quickshort:** our `RecStop` (dot + `REC m:ss` + Stop) is
the right shape — reskin to tokens; consider *also* a thin `--red` 1.5px frame
around the recorded region (GNOME's area indicator) so the user sees what's
being captured; keep right-click→hide as a later nicety.

### 3.9 Cross-platform built-ins

- **KDE Spectacle (new QML UI):** rectangle mode overlay gets a **vertical left
  toolbar of tools + bottom toolbar of tool options** once you annotate,
  undo/redo on a top bar, `Magnifier` shown only while *no* tool is armed, and
  annotations carry over into the viewer window (undo still works after
  accepting). A `FloatingToolBar.qml` exists. [plasma/spectacle commits,
  docs.kde.org]
- **Windows Snipping Tool:** post-capture editor (pen/highlighter/eraser/
  shapes/emoji/crop/Text Actions OCR); **Sept 2025 "Quick markup"** — toggle
  `Ctrl+E` and the pen/highlighter/eraser/shapes toolbar lives **inside the
  selection overlay before the capture commits**, with grabbers for re-cropping.
  This validates Quickshort's annotate-in-overlay model on Windows.
  [Microsoft support, Windows Insider blog 2025-09-17]
- **GNOME Screenshot:** overlay with handles + crosshair, Screen/Window/
  Selection segmented buttons, pointer-include toggle, big round capture button,
  red recording indicator. [help.gnome.org]

**What adapts vs stays:** the design *tokens* stay identical on all three OSes
(black raised surface, hairline, accent). Per-OS adaptations: modifier glyphs in
key hints (⌘/⌥ vs Ctrl/Alt — config already records "CmdOrCtrl+"); popup dock
margins (macOS needs +44px top inset for the menu bar — already handled in
`widgets::dock_pos`; Windows bottom-right should clear the taskbar corner);
tray context menu stays native per toolkit (muda today); titlebar/traffic
lights only in settings, and only where the platform expects them (Windows gets
a ✕ at top-right instead of lights).

### 3.10 OSS clones worth a glance

- **Screenshotli** (treeinspired.com): "floating toolbar appears right under
  your selection with every tool one keystroke away; resize the captured area
  with corner handles and it **re-captures instantly**" — handle-drag =
  re-grab is a behavior worth copying.
- **Capso** (GitHub): CleanShot-style all-in-one HUD; `R` cycles aspect-ratio
  presets on the marquee; dimension display during drag.
- **DodoShot** (GitHub): compact corner overlay, **auto-dismiss pauses on
  hover**, pins with opacity + click-through.
- **tyypgzl/screenshot** (GitHub): 30-color picker, badge auto-increment, text
  commits on Enter/cancels on Esc — same semantics as ours.
- **StickyShot** (GitHub): pin windows with scroll-to-adjust opacity.

---

## 4. Detailed spec blocks

All sizes logical px. Colors use ui-2026 tokens. These are implementation-ready:
they reuse the existing `sdf` primitives and hit-test structure.

### 4.1 Selection overlay

```
dim        : #000 @ 50% outside selection (keep current >>1 math)
crosshair  : horizontal + vertical hairlines through cursor
             stroke #ededed @ 12%, 1 phys px, gap 6px around cursor
             hidden once selection confirmed
selection  : interior un-dimmed (exists)
             border while dragging: dashed --accent, 1.5px, dash 8 / gap 4
             border confirmed: solid --accent 1.5px + handles
handles    : 11×11 squares, #000 fill + 1.5px --accent border,
             at corners + edge midpoints (8 total); hover → --accent fill;
             drag = resize (Screenshotli: re-grab = re-render undimmed region)
size chip  : mono 11px, "1280 × 720", --ink on #0a0a0a + 1px #262626, r6,
             padding 6×10; docked 6px above selection top-left;
             flips below-left when clipped; during drag → follows cursor
             offset (+14,+18); click toggles logical ⇄ physical px
min-select : keep 3px; below it, chip hides (no 0×0 flicker)
esc        : cancel; also click-without-drag (exists)
```

### 4.2 Annotation toolbar — layout

```
geometry   : keep R=14 discs, GAP=8, PAD=6, EDGE_GAP=4 (toolbar.rs:88-91)
dock order : bottom → top → right → left → inside-bottom-right (keep)
shape      : r = 14px per button (discs) — OR variant B: one r8 pill bar;
             recommendation: keep discs, they're already SDF-rendered and
             disc = the "button" vocabulary of the overlay, bar = nothing else
separators : 1px #262626 divider, 18px tall, centered, 4px gap each side
             between groups (below)
disc       : fill #0a0a0a @ 92%, rim 1px #262626 (was 12% white),
             soft shadow: #000 @ 24%, offset y+1.5, 4px falloff (exists)
hover      : fill #1a1a1a, rim #3a3a3a
active     : fill --accent, glyph #000 (currently accent fill + white glyph —
             swap to black glyph on accent for ui-2026 contrast)
disabled   : glyph @ 35% (exists)
glyph      : SDF stroke 1.8px --ink (exists)
tooltip    : on 400ms hover — mono 10px chip below/above the bar:
             name + key, e.g. "ARROW  A" / "SAVE  ⌘S"; #0a0a0a+#262626, r6
```

**Order + grouping** (16 items today → same count, three groups):

```
[ Select(V) ] ─ group: manipulation
[ Pen(P) Arrow(A) Rect(R) Ellipse(O) Marker(M) Text(T) Pixelate(X) ] ─ draw
─────────────────────────── separator ───────────────────────────
[ Undo(⌘Z) Redo(⌘⇧Z) ]
─────────────────────────── separator ───────────────────────────
[ Save(⌘S) Copy(⌘C) OCR(⌘T) Share(⌘L) | Scroll(⌘G) Record(⌘R) ]
```

Current order is identical (Select,Pen,Arrow,Rect,Ellipse,Marker,Text,Pixelate,
Undo,Redo,Save,Copy,Scroll,Record,Ocr,Share) — only Ocr/Share move left into
the primary action quartet and two separators get drawn. Grouping rule stolen
from ShareX's taxonomy: *manipulate | draw | history | terminal*.

**Options row** (armed tool only, exists — keep geometry):

```
stroke tools : [S ●][M ●][L ●]  sep  [8 swatch discs r6]
text tool    : [12][14][18][24][32][48][B]  sep  [8 swatches]   (exists)
swatch sel   : 1.5px --accent ring outside the disc (not fill — survives white
               and black swatches)
new: wheel   : scroll while tool armed = cycle size (Flameshot)
new: r-click : radial palette at cursor, 8 wedges (Flameshot) — optional v2
```

### 4.3 Capture stack (post-capture album — replaces the quick-access card)

```
flight     : on toolbar action commit, the captured region animates to the
             popup_pos corner — ~550ms spring, scale→thumb; the flight IS
             the success confirmation (no "saved" toast needed)
window     : always-on-top, borderless, r8 clipped thumbs; docked at
             popup_pos on the *capture's* monitor (fix widgets.rs
             popup_monitor — Shottr parity)
thumb      : 104×66 (or aspect-fit), r6, 1px #262626 border, cover-crop
stack      : newest on top, offset ~6-7px per older shot; max ~5 visible,
             older collapse; count chip "N SHOTS" mono 10px when >1
hover      : fans out horizontally; each thumb gets accent ring + × delete
click      : reveal/open the shot (saved file or held image)
drag       : thumb is a promised-file drag source into any app
r-click    : actions menu — Reveal in Finder · Copy image · Share link ⌘L ·
             Beautify · Pin to screen · ─ · Delete (red)
badge      : 15px circle bottom-left of the top thumb:
               ✓ --grn saved/copied · ⧉ --accent link copied
               ◌ --accent spinning while uploading · ✕ --red failed (click retries)
lifetime   : persists until used; no countdown — the toolbar already did
             the action, the stack is a shelf not a decision prompt
states     : uploading thumb shows ◌ badge overlay → flips to ⧉ / ✕;
             one-shot --capture exits after the flight (process needn't
             stay alive — current constraint stays)
```

### 4.4 Toast variants

```
window     : ~300×64, r11, AlwaysOnTop, docked popup_pos, offset monitor-aware
layout     : [30×30 icon cell] x=12, cy=center; title 14px/550 --ink;
             mono sub 11px --muted (existing .toast spec verbatim)
icon cell  : r6, 1px tinted border, glyph mono 13px:
               ok   : ✓ --grn, border rgba(48,209,88,.45)
               info : ⧉ --accent, border rgba(82,199,234,.45)
               work : ◌ --accent spinning 1.2s linear
               err  : ✕ --red, border rgba(255,95,87,.45)
countdown  : 2px strip bottom, --grn ok / --red err / --accent work
TTL        : 1.8s done, until-done while working (exists)
dismiss    : click OR drag-away ≥40px (Shottr)
variants   : Publishing… / OCR working / Share failed + "retry ⌘L" in sub —
             success states live on the stack badge, not in toasts (§4.3)
```

### 4.5 REC dock

```
window     : 172×46 → keep; r11; docked popup_pos (top-right default)
chrome     : #0a0a0a, 1px #262626, black fringe
dot        : --red r5, pulse a ∈ [.55,1] @ ~1.5Hz (exists @3.0 rad/s — fine)
label      : "REC 0:07" mono 12px --ink, at x=26
stop       : pill (108,10,52,26) r13; default #1a1a1a + #262626 + --ink;
             hover --red fill + #000 label (exists as color swap — keep)
optional   : pause pill appears left of Stop if recording gains pause
             (Screen Studio parity) — reserve x=52 for it
v2         : 1.5px --red frame on the recorded region itself (GNOME indicator)
```

### 4.6 Settings rows (dark reskin of existing window)

```
window     : 820×600; sidebar 200px #050505, content #000
cards      : #0a0a0a, 1px #262626, r8; rows 52px (exists)
row        : icon tile 28px r6 #1a1a1a + --muted glyph; label 13px --ink;
             desc 11px --muted (was FG/FG_DIM on white — same geometry)
switch     : 32×18; off #262626; on --accent fill, knob #000→#fff? use:
             off: #262626 track + #6b6b6b knob; on: --accent track + #000 knob
keycap     : .kbd-inline spec: mono 11px, #0a0a0a, 1px #262626, r5, pad 4×9;
             recording state: accent border + accent text "PRESS KEYS"
swatches   : accent picker = ACCENTS discs r7 + 1.5px accent ring on selected
dropdown   : #0a0a0a, 1px #262626, r6, 12px --ink label + ▾ --faint;
             open list = same chrome, hover row #1a1a1a
nav        : sidebar row h34, active = #1a1a1a + accent 2px left bar;
             glyph+label 12px
danger     : "Reset…" → --muted; confirm state "Confirm?" --red
about      : version chip .chip.ok "V0.2.1 ✓" → update .chip.acc "UPDATE →"
```

### 4.7 Tray menu (native per-OS — keep)

```
1  Capture Region          ⌘⇧S      (shows live config hotkey — exists)
2  Recent Captures ▸                (submenu, disabled when empty — exists)
3  ─────────
4  Open Screenshots Folder
5  Settings…
6  Launch at Login            ✓
7  ─────────
8  Update to vX — Restart to Install / Check for Updates…
9  Support Quickshort
10 Quit Quickshort                  ⌘Q suggested
icon: SDF crop-corners glyph (exists) — monochrome template on macOS,
      --accent detail permissible on Linux/Windows tray
```

---

## 5. Sources

Fetched/searched (all URLs real, as visited this session):

- https://cleanshot.com/features — QAO feature list (drag-drop, auto-close, position/size config, swipe gestures), annotate bullets, advanced capture (crosshair+magnifier, freeze), recording (dimming option, menu-bar timer, mic/system audio, auto-DND)
- https://cleanshot.com/screenshots — annotate/redact tool framing, counters, CleanShot editable file format
- https://cleanshot.com/docs-api — settings tabs (general/wallpaper/shortcuts/quickaccess/recording/screenshots/annotate/cloud/advanced/about), capture-area actions
- https://www.macworld.com/article/617854/cleanshot-x-review.html — QAO "command center" description, corner copy/save buttons, counter tool, editable annotations vs Markup
- https://www.imore.com/cleanshot-x-gains-new-keyboard-shortcuts-quick-access-overlay-more — QAO keyboard shortcuts, "Move to trash", close-button relocation, recording dimming option
- https://defkey.com/cleanshot-x-shortcuts — full annotate keymap (V K D M L T A C E P H R F B) + ⌘ shortcuts
- https://alchemists.io/articles/clean_shot — annotate keys, recording dim/red-border behavior, QAO hover-only shortcuts (search snippet; page fetch returned binary garbage)
- https://typenorm.com/apps/cleanshot — "one more thing hovering" critique (fetched)
- https://github.com/xkmttc/Capso — CleanShot-style HUD clone description: R cycles aspect ratios, dimension display, quick-access pill inventory
- https://shottr.cc + https://shottr.cc/newversion.html — 16 tools when window wide, drag-up dismiss, menubar confirmation animation, dimension box click-to-toggle logical/physical, ruler keys, ⌘+[ ] resize, scroll-resize pins
- https://shottr.cc/kb/precision — arrows/⌘-arrows/Shift-10px nudging, Q/W corner zoom, Z+drag, ⌘2 zoom-to-selection
- https://shottr.cc/kb/faq — rasterize flow, editor-opens-on-capture-screen
- https://www.bitdoze.com/shottr-mac-screenshot-tool/ — 16-tool inventory (bendable arrows, spotlight 1-9 opacity, counter, magnifier), smart-select ⌥ padding balance
- https://www.howtogeek.com/reasons-i-use-shottr-instead-of-the-mac-screenshot-tool/ — ruler `1`/`2`, add/paste image buttons, color picker
- https://www.screensnap.pro/blog/shottr-mac-review — "bare" UI critique vs CleanShot QAO polish
- https://chrisdermody.com/product-review-shottr-screenshot-for-mac/ — single-key tool usage, ⌘[ ] resize
- https://buttondown.com/shottr/archive/shottr-v153-is-out/ — preview-thumbnail option, Enter→editor
- https://support.apple.com/guide/mac-help/take-a-screenshot-mh26782 — ⇧⌘5 panel, Options menu inventory, floating-thumbnail behaviors
- https://macmost.com/live-using-the-screenshot-tool-on-your-mac.html — toolbar modes, ~5s thumbnail TTL, Enter=capture, trash semantics
- https://www.techsmith.com/learn/tutorials/snagit/snagit-themes/ — themes → auto-generated Quick Styles
- https://www.techsmith.com/learn/tutorials/snagit/customize-toolbar/ — toolbar customization, groups, icon-only
- https://www.techsmith.com/learn/tutorials/snagit/recent-captures-tray/ — recents tray, pinning
- https://flameshot.org/docs/guide/key-bindings/ — full local keymap (p d a s r c m t b g, Space side panel, right-click color wheel, wheel thickness, Ctrl/Shift resize modifiers, Enter=upload)
- https://github.com/flameshot-org/flameshot/blob/da6121bd/src/widgets/panel/sidepanelwidget.cpp — side panel contents: size spinbox+slider, active color, color wheel, hex input
- https://github.com/flameshot-org/flameshot/blob/da6121bd/src/widgets/capture/capturewidget.h — magnifier, xywhTick, notifier box, update notification widget
- https://www.howtogeek.com/devops/how-to-use-flameshot-a-linux-screenshot-tool/ — side-panel usage, history-revert entries, theme/backdrop-opacity settings, button-disable list
- https://getsharex.com/docs/image-editor.html — bottom toolbar terminal actions, Enter=continue-task, V/R keybinds
- https://github.com/ShareX/ShareX.ImageEditor — Avalonia editor, annotation inventory, host callbacks
- https://deepwiki.com/ShareX/ShareX/3-screen-capture-system — RegionCaptureForm modes + ShapeType taxonomy (Region/Drawing/Effect/Tool)
- https://xnapper.com/ + https://xnapper.com/changelog + https://xnapper.com/blog/annotate-screenshot — auto-balance/background/redact, padding/inset/shadow/rounded click-to-type values, "Drag me" button, blue selection rings, social ratios
- https://daveswift.com/xnapper/ — editor layout (right controls, presets)
- https://screen.studio/guide/recording-area + https://preview.screen.studio/guide/managing-recording-in-progress + /starting-finishing-the-recording — recording control widget (time, finish/pause/restart/delete, right-click hide), 4 stop paths
- https://help.gnome.org/gnome-help/screen-shot-record.html — GNOME overlay (handles, crosshair, big round button, mode buttons) + top-right red REC indicator w/ elapsed seconds
- https://invent.kde.org/plasma/spectacle/-/commit/9d4ffdf70e5a29fa9105cbf8c4f7e1ddca9e377a — Spectacle QML rework: vertical left tool bar + bottom options bar + top undo/redo, FloatingToolBar/Magnifier QML
- https://invent.kde.org/plasma/spectacle/-/commit/0747560cc8a2f7781f091f7f65b6694b567d8173 — magnifier only when no tool armed
- https://docs.kde.org/trunk_kf6/en/spectacle/spectacle/additional-functionality.html — annotation editor description
- https://blogs.windows.com/windows-insider/2025/09/17/paint-snipping-tool-and-notepad-app-updates-begin-rolling-out-to-windows-insiders/ — Quick markup in-overlay toolbar (Ctrl+E), grabbers re-crop
- https://support.microsoft.com/en-us/windows/apps/use-snipping-tool-to-capture-screenshots — editor inventory, Perfect Screenshot handles
- https://treeinspired.com/screenshotli/ — floating toolbar under selection, one-key tools, re-capture on handle resize
- https://github.com/bluewave-labs/DodoShot — hover-pauses auto-dismiss, pin opacity/click-through
- https://github.com/tyypgzl/screenshot — bottom toolbar, 30-color palette, Enter/Esc text commit
- https://github.com/rgcr/stickyshot — scroll-opacity pins
- https://manual.raycast.com/settings — settings IA (menu-bar toggle, theme, interface size)
- https://www.raycast.com/blog/a-fresh-look-and-feel — bottom action bar consolidating actions+toasts
- https://developer.apple.com/documentation/AppKit/NSStatusBar + HIG menu-bar extras notes — use sparingly, symbolic icon, removable

Local files: `demo/ui-2026.html` (token source), `demo/design-research.md` (landing-side
companion; its §4.2 in-app sketch is superseded/extended here), `docs/` landing CSS,
and all `crates/` sources listed in the header.
