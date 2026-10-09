# Quickshort — In-App UI Research, Round 2

Deeper interaction findings that `app-ui-research.md` (round 1) does **not**
cover. Round 1 decided the *look* (dark tokens, dashed selection, grouped
toolbar, capture stack concept); this round decides the *behaviors*: stack
interaction model, overlay micro-UX, tool gaps, recording/scroll chrome, pin
physics, first-run and accessibility.

Code refs: `crates/qs-overlay/src/lib.rs` (overlay), `crates/qs-app/src/`
(`quick_access.rs`, `pin.rs`, `recstop.rs`, `toast.rs`, `settings.rs`,
`tray.rs`, `scroll.rs`, `record.rs`, `host.rs`, `widgets.rs`, `config.rs`),
`demo/desktop-sim.html` (prototype).

Priorities: **P0** = do for the stack release · **P1** = next milestone ·
**P2** = opportunistic. Every competitor claim carries a source; `[?]` marks
anything unverified.

---

## TL;DR — top 10 UX deltas

1. **Give the stack a keyboard and a peek.** macshot (CleanShot clone) already
   ships the exact model: arrow keys navigate a capture-history overlay,
   `Enter`/`⌘C` copy, `⌘E` opens editor, `Space` = Quick Look, `Delete`
   removes — and mouse hover *syncs* the keyboard selection so both inputs
   share one selection ring. CleanShot QAO 2.0 independently added Space =
   Quick Look. Our stack is currently mouse-only; this is the single biggest
   interaction gap. **P0** — cheap (the stack is one softbuffer window).
2. **Drag-out is the stack's killer feature — and winit can't do it.** Every
   pro tool's post-capture surface is a *promised-file drag source*
   (CleanShot QAO, macOS floating thumbnail both drag into Finder/editors).
   winit has no outgoing-drag API; we need per-OS file-promise code (macOS
   NSPasteboard file promises, Windows `CF_HDROP`/`IDropSource`, X11
   XdndDirectSave / Wayland `wl_data_source`). Ship macOS first. **P0.**
3. **Click = preview, not editor.** macOS (click→Markup), iOS (tap→Markup)
   and CleanShot (click→Annotate) all send click to an editor — but CleanShot
   got there via a *light* preview first (QAO 2.0 added Quick Look). For a
   "shelf not decision prompt" the least-surprising ladder is: click =
   lightbox preview (sim already does this), Space = same, `⌘E`/double-click
   = annotate. Don't open a heavy editor on click. **P0** (it's a policy
   decision, zero code).
4. **Persist the stack across restarts by rehydrating from disk.** CleanShot
   ships "Restore recently closed overlay" *and* a 1-month Capture History
   with type filters — the stack is a cache, the disk is the truth. On
   launch, repopulate the stack with the N newest files in `save_dir`
   (config `recent_max` already exists). Plus a tray/menu item "Reopen last
   capture". **P0/P1.**
5. **Recording chrome: take CleanShot's pause + live stats.** CleanShot has
   pause/resume, restart, trash-with-confirm *hotkeys*, plus file-size and
   audio-source info live on the video overlay — and its finished recording
   lands in the QAO. Our `RecStop` is Stop-only but `host.rs` already drops
   recordings into the card (`RecDone → open_card`). Spec: `REC 0:07 ·
   2.1 MB` + pause pill, recording thumbs in the album with a ▶/duration
   badge. **P1** (pause), **P0** (album thumb + duration label).
6. **Scroll capture needs chrome — borrow the REC dock.** Snagit's
   Panoramic toolbar (Start/Stop), CleanShot's Start Capture → "scroll
   slower" warning → Done, and Shottr's auto-scroll-with-"don't touch
   anything" all agree: show live progress and a stop path. Quickshort polls
   silently for up to 90s with zero UI (`scroll.rs` knows shots-taken, total
   height, and exactly when stitching fails — `MIN_OVERLAP` mismatch, idle
   stop, 24-shot cap). Show a `SCROLL · 3 shots · 4,200 px` dock + Stop. **P0**
   — it's the only mode with no feedback at all.
7. **Esc layering is right; selection needs the missing modifiers.** Our Esc
   chain (text draft → grabbed annot → selected annot → cancel, lib.rs:
   1508-1563) is already correct. What's missing is drag-phase modifiers:
   macOS's own contract (Space moves the marquee, Shift locks one axis,
   ⌥ resizes from center — documented in mh26782) plus Flameshot's
   Ctrl=keep-aspect. And arrow-key nudging (Shottr: 1px, ⇧10px, ⌘ resize)
   once a selection exists. **P1** — modifiers first, nudge second.
8. **Pin windows need the ShareX control set.** ShareX's documented pin
   spec is the best anywhere: wheel = scale 20–500%, Ctrl+wheel = opacity
   10–100%, middle-click = reset, double-click = collapse to mini-preview,
   arrows = 1px move / Shift 10px, right-click = close. Shottr adds
   scroll-resize and drops the shadow on translucent pins. Our pin is
   fixed-size drag+× (pin.rs). Wheel-scale + Ctrl-wheel-opacity + middle-
   reset is ~80% of the value. **P1.**
9. **First-run = permission preflight + a taught hotkey.** macOS Screen
   Recording TCC has a brutal invariant: ScreenCaptureKit won't work in the
   process that received the grant — you must quit & relaunch, and
   `CGPreflightScreenCaptureAccess` can return a stale false right after
   (both documented in OSS bug reports). Do a *silent* preflight in a
   one-time onboarding card, request behind a button, and show the "Quit &
   Reopen" state explicitly — otherwise first capture fails with a cryptic
   black shot. **P0** for macOS.
10. **Respect reduced-motion + non-QWERTY.** Kill the fly-to-corner under
    `accessibilityDisplayShouldReduceMotion` (fade instead); on Windows map
    to `SPI_GETCLIENTAREAANIMATION`. And our single-char tool keys + mono
    `⌘S` chips must (a) render Ctrl/Alt on Win/Linux and (b) survive Dvorak/
    AZERTY — global-hotkey conflicts need a "this combo is taken" row state
    (pot-desktop classifies failure: free / held-by-other-app / OS error).
    **P1.**

---

## 1. Capture stack / album — interaction model

Round 1 committed the *direction* (passive album, flight = confirmation).
These are the mechanics it didn't pin down.

### 1.1 Fan vs. click, max visible, overflow

- **CleanShot QAO doesn't stack at all** — each capture spawns a *separate*
  overlay, stacked vertically in the corner; position, size and auto-close
  are configurable and it supports multi-display. Multiple captures = multiple
  cards, no fan, no count chip. Changelog notes "subtle indicator for newest
  screenshot" and "improved interaction when closing multiple files one by
  one". [cleanshot.com/features, cleanshot.com/changelog, macworld review]
- **iOS** stacks repeated screenshots into one thumbnail pile; a left-swipe
  dismisses the *whole stack*, tap opens all of them in Markup together;
  rapid re-capture auto-dismisses the visible thumbnail.
  [osxdaily.com, ios.gadgethacks.com, discussions.apple.com]
- **macshot (OSS, closest model to ours)** has a history overlay with full
  keyboard nav and *hover-synced selection*: "hover and keyboard selection
  share a single outlined-card state". [macshot CHANGELOG v4.1.2]
- **Slack composer** stages attachments as a horizontal strip of chips —
  per-file spinner while uploading, error chip with retry on failure, remove
  button on hover; drag-reorderable. [slack.com/help 201330736]

**Quickshort today:** sim fans horizontally on hover (`translateX(-128px)` per
index), hard-caps at 5 thumbs (oldest dropped), `N SHOTS` count chip, no
overflow story, no keyboard. [desktop-sim.html:165-192, 484-494]

**Spec:**
- Fan on hover stays (it's delightful and already works); **click on the
  count chip toggles a pinned "expanded" state** so the fan survives
  pointer-leave — this replaces the missing click-expand and doubles as the
  keyboard-focus target. **P0**
- Cap visible thumbs at 5; beyond that, items don't vanish — the stack
  pins oldest-first offscreen *behind* the chip and the chip reads `8`.
  Never silently drop a capture (the sim literally `remove()`s it).
  **P0 — fix.** A capture the user can't find again is a data-loss bug.
- Thumb sizing: cover-crop is right for normal shots but **letterbox
  (aspect-fit) for extreme aspects** — a 800×20000 scroll capture
  cover-cropped to 118×74 shows only a sliver. Rule: cover-crop when
  `aspect ∈ [0.5, 3]`, else aspect-fit on a `#050505` mat. **P1**
- Auto-collapse: keep "persists until used" (round-1) but steal CleanShot's
  **slide-down-to-hide**: drag the stack ≥40px toward its screen edge → it
  collapses to just the count chip; chip click restores. Cheaper than
  dismiss and reversible. **P1** [cleanshot.com/changelog — "temporarily
  hide Quick Access Overlay by sliding it down"]
- Position: `popup_pos` already gives 6 corners (config.rs:147). One
  correction — the stack should dock on the **monitor that produced the
  capture**, not `primary_monitor()`; `widgets::popup_monitor` (widgets.rs:14)
  is documented wrong for this. CleanShot's "Multi-display support" is the
  cited precedent. **P0** (same bug hits toast/REC dock).

### 1.2 Drag-out semantics (the hard part, and the payoff)

- **CleanShot:** the QAO thumbnail is a live drag source into any app or
  Finder, pre-save; drag does not close the overlay.
  [cleanshot.com/features "Drag & Drop to any app", macworld]
- **macOS floating thumbnail:** drag into a document/email/Finder during its
  ~5s TTL (TTL itself is tunable via `defaults write com.apple.screencaptureui
  thumbnailExpiration`). [support.apple.com mh26782, stackoverflow 74140663]
- **Engineering reality:** winit `0.30` has no outgoing drag-and-drop — a
  drag that leaves our window needs platform code:
  - macOS: `NSView` file-promise dragging source
    (`NSFilePromiseProvider`/`filePromiseProvider:fileNameForType:`) so the
    receiver materializes the PNG lazily — matches our "image exists in
    memory, may not be on disk" model.
  - Windows: `IDropSource`/`IDataObject` with `CF_HDROP` (saved file) or
    `CFSTR_FILEDESCRIPTORW`+`CFSTR_FILECONTENTS` (in-memory).
  - Linux: X11 `XdndDirectSave` or `text/uri-list`; Wayland `wl_data_source`
    offers `image/png` + `text/uri-list`. [winit gap: `?` — no winit API
    covers this; Tauri's `drag` crate is the usual workaround]

**Spec:** thumb press-and-hold ≥150ms or move ≥6px = begin drag; drag shows
a lifted thumb under the cursor (sim: opacity .85, scale 1.05); drop on
Finder/app saves-then-delivers; drop nowhere = snap back, stack unchanged;
drag *out of the stack region then release on desktop* = also valid "save to
Desktop". **P0 on macOS, P1 Win/Linux.** Also: drag thumb **onto another
app window via its taskbar/dock icon** is standard promised-file behavior —
free once the source is a real promise.

### 1.3 Click & keyboard semantics

- **Click → preview** (lightbox exists in the sim). CleanShot click =
  annotate, but macshot's split is the better steal: `Space` = Quick Look,
  `⌘E` = editor, `Enter`/`⌘C` = copy, `Delete` = remove, arrows = move
  selection. **P0** — mirror this set 1:1; it maps onto actions we already
  have (Copy/Share/Beautify/Pin/Delete).
- **Focus acquisition:** hovering the stack gives it keyboard focus *while
  hovered* (CleanShot 4.1 added hover-keyboard-shortcuts to the QAO the same
  way [imore]). Plus a tray/global "Show capture stack" path so it's
  reachable without a mouse — even if that's just ⌘⇧C focusing the stack.
- **Undo a delete:** `⌘Z` while stack focused restores the last removed
  thumb (in-memory only; file deletes stay immediate + unrecoverable, same
  as CleanShot "Move to trash"). **P2** — but make `×` destructive-but-
  undoable for *unsaved* thumbs, since those truly vanish otherwise.
- **Retry on failure:** badge `✕` click = retry upload (round-1 spec); steal
  Slack's richer pattern — during upload show a **2px progress bar along the
  thumb's bottom edge** instead of only the ◌ spinner, and on failure make
  the thumb show `✕` badge *and* hover label "Upload failed — click to
  retry". [Slack chips show per-item progress; assistant-ui/ag-ui trays
  document the queued→uploading→error-with-retry state machine]

### 1.4 States, queue, persistence

- Uploading stays interactive: a mid-upload thumb is still draggable (share
  the local file), still previewable. Only *Share-dependent* actions wait on
  the upload. Slack's "send while pending" middleware rationale applies.
  [getstream pending-uploads doc]
- **Session restore:** on app launch repopulate from `save_dir` newest-first
  up to `recent_max` (default 8). Badge = `✓` (they're saved files). Add
  tray item "Reopen last capture". This is CleanShot's "Restore recently
  closed overlay" + Capture History boiled down to a diff. **P0** (it's
  `readdir` + sort).
- Recordings land as stack thumbs too (CleanShot puts recordings in the
  QAO; we already route `RecDone → open_card` in host.rs:596) — badge `▶`
  + duration label in mono, click previews/plays. **P0** in the new design.
- Scroll captures land as tall thumbs (letterbox, see 1.1) — the "shape is
  the metadata" principle: tall thin thumb ⇒ scroll shot, no label needed.

---

## 2. Overlay micro-UX

### 2.1 Drag-phase modifiers (macOS's own contract — we're missing all of them)

While a marquee is live, macOS defines: **Space** drags the whole selection,
**Shift** locks/adjusts a single axis, **⌥** resizes from the center
[support.apple.com mh26782]. Flameshot adds **Ctrl**+drag = keep aspect,
**Shift**+drag = mirror-resize [flameshot key-bindings]. CleanShot's
All-In-One offers aspect-ratio lock + remembered last selection
[cleanshot.com/features]; Capso cycles aspect presets on `R` [round-1].
Quickshort has **none** of these — `mods` is tracked but only used for
text editing and ⌘-actions (lib.rs:386, 1474).

**Spec (P1):** during drag — `⌥` = resize from anchor center; `⇧` after
first motion = constrain to horizontal/vertical; `Space`+drag = move the
rect (keeps size). While *confirmed* — `⇧` on a handle drag = preserve
aspect (cheap: lock `w/h` ratio at grab time). `R` cycles aspect presets
(16:9 → 4:3 → 1:1 → free) shown as a chip next to `W×H`. Conflict note:
`Space` currently paper-patches the winit IME-leak (lib.rs:1477-1497) —
gate the move-selection grab on `self.active.is_none()`.

### 2.2 Nudging + remember-last-selection

Shottr's nudge set — arrows 1px, `⇧`arrows 10px, `⌘`arrows resize, `[` `]`
symmetric — [shottr.cc/kb/precision]. None exists today; our selection is
mouse-only once confirmed. **P1**: arrows move the confirmed selection 1px
/ ⇧10px / ⌘arrows resize from the SE corner. Remember-last-selection is a
hotkey-level thing, not overlay state: CleanShot's "Capture Previous Area"
and macOS's `last-selection` plist (com.apple.screencapture defaults — real,
[LnL7/nix-darwin#965 dump]) both persist the rect across invocations. **P1**:
store last rect in config, `⌘⇧D`-style "recapture last area" skips the
overlay entirely.

### 2.3 Magnifier — worth it, Phase 2

- **Flameshot**: optional circular *or square* magnifier widget
  (`MagnifierWidget(pixmap, color, isSquare)` — `squareMagnifier` config),
  hidden while the move tool is active; comes from the same code as the
  color-grab zoom. [flameshot.org dev docs, PR #2219, capturewidget.cpp]
- **CleanShot**: magnifier inside crosshair mode (advanced prefs); ⌘ shows
  crosshairs while dragging. [cleanshot.com/changelog]
- **Spectacle**: magnifier shown only while *no* tool is armed — the clean
  rule. [plasma/spectacle commit 0747560]
- **Shottr**: magnifier is an *editor callout tool* (zoomed-in inset), not a
  selection aid — different beast. [shottr.cc changelog]

**Spec (P2):** 96px circle, 8× nearest-neighbor zoom of the un-dimmed
composite, crosshair pixel highlighted, lives 16px from the cursor opposite
the size chip; shown only when no tool armed (Spectacle's rule); off during
annotation drags. The composite already exists (`composite()` output) so
it's a blit + circle clip — cheap in software. Do the crosshair first (P0,
round-1), magnifier only if users ask.

### 2.4 Window auto-detect — skip for v1, note the shape

macOS Space→camera-mode and Snagit's scrolling arrows are *different modes*,
not hover-detection inside region mode. CleanShot's All-In-One does hover-
highlight windows. Real window geometry requires Accessibility API on macOS
(`AXUIElement` frame queries — a second permission we don't currently ask),
or platform window lists elsewhere. **Verdict: P2** — if done, it's a mode
toggle (`Space` pre-drag), needs the AX permission bundled into first-run,
and must draw the detected frame on hover *before* click. Don't half-ship
hover detection without the AX grant path.

### 2.5 Tiny-selection & screen-edge cases

- Sim and app both clamp the toolbar (cascade in toolbar.rs:143). Additional
  rule from Flameshot's `xywh_position` config: the **geometry readout can
  be placed *inside* the selection** when there's no room outside — our
  `W×H` chip needs the same "dock inside-center" fallback for selections
  hugging screen edges. [flameshot#3207 discussion]
- **Handles overflow inward**: below ~60px selection width the 8 handles
  (round-1 spec, 11px) crowd; draw handles *inside* the rect when
  `w<60||h<60` — they're still grabbable, they just overlap the content.
  Standard canvas-editor move; no competitor citation needed.
- Sub-`MIN_SELECT` (3px) drags currently cancel the overlay entirely
  (lib.rs:1452 `finish(None)`) — a stray click nukes the session. Compare:
  macOS treats click-without-drag as cancel too, but a **2-5px accidental
  drag shouldn't exit**; only a true click (no movement) should. Keep 3px
  for the selection, but require `moved==false` for cancel. **P1 — cheap
  fix, real footgun.**

---

## 3. Toolbar & annotation tools

### 3.1 Counter tool — the signature gap (P1)

CleanShot's counter drops auto-incrementing numbered circles on each click;
recent versions added *counter style + starting number* (including 0) and
made counter objects always render on top. Every review names it as the
annotating-tutorial tool. [cleanshot.com/changelog, cleanshot.com/screenshots,
macworld]. ShareX calls it "step numbers" in its Drawing taxonomy [round-1].

**Spec:** `N` key; click places a filled disc (accent bg, `#000` number,
mono) auto-incrementing per image; options row gets `[n=] [style: disc |
square]`; always-on-top z (they're labels, not markup). Fits our
`Annotation` enum as `Counter { p, n, r, rgb }`.

### 3.2 Spotlight & highlight upgrades (P1)

- **Spotlight**: dim everything *except* a dragged region. Shottr lets keys
  `1..9` set the dim opacity while armed — steal verbatim (free: map key to
  α = n/10). [shottr.cc changelog "Spotlight tool (change background opacity
  with keys 1..9)"]
- **Highlight/marker**: CleanShot's newer Highlight tool has *rounded
  corners and ellipse shape* options — i.e. a highlighter that isn't stuck
  as a fat diagonal line. Our marker is already a soft-edged stroke; the
  delta is a rounded-rect variant. **P2.** [cleanshot.com/changelog]

### 3.3 Blur vs. pixelate (P1 policy decision)

Shottr ships *both* blur and erase (erase text outright); we ship pixelate
(`X`). Add soft **gaussian-ish box blur as a second option on the same
button** (long-press/right-click the disc or a second options-row chip:
`PIXEL | BLUR`). Rationale: pixelation of *short text* is partially
reversible (public "depixelization" research exists — `[?]` mark; cite
depix-style repos if we want to justify it in-app). For *redaction* a solid
black rect beats both — our Rect+fill already does that; surface it as a
"REDACT" preset chip in the options row rather than a new tool.

### 3.4 Per-tool last-used style (P1)

macshot's changelog describes the *bug* of not persisting it —
`lastUsedTool` + `currentStrokeWidth` kept getting reset — which confirms
the feature itself: per-tool persistent style. Our options row already
selects stroke/color per tool; persist `(tool → size,color)` into config so
arming Pen next session restores your 3px red. Snagit's Quick Styles is the
full-fat version; persistence is the 80% case. **Spec: one TOML table.**

### 3.5 Toolbar at screen edge — two small upgrades

Cascade (bottom→top→right→left→inside) is fine; add from desktop-sim
behavior (which already guards `innerHeight-60`): (a) when docked *inside*
the selection bottom-right, offset inward so hover doesn't repaint the
selection edge; (b) when selection is narrower than the bar, let the bar
overflow the selection horizontally but clamp to the *monitor* — already
done, keep. No competitor does better than the cascade; skip re-litigating.

---

## 4. Recording & scroll capture UX

### 4.1 Recording chrome (P1)

| Feature | Competitor | Quickshort today |
|---|---|---|
| Pause/resume | CleanShot has it as a *hotkey* (Pause/Resume Recording, customizable) + on-widget | none — recstop.rs is Stop-only |
| Restart take | CleanShot + Screen Studio (restart = fresh take without menu) | none |
| Live stats | CleanShot shows **file size + sound info** on the video overlay [changelog] | none |
| Menu-bar timer | CleanShot "Display recording time in the menu bar" [features] | none — REC dock only |
| Camera bubble | CleanShot: position/size/shape/fullscreen, Continuity Camera as source [daveswift review] | none |
| Pre-countdown | macOS self-timer (5s/10s), CleanShot Self-Timer [features] | none |

**Spec:** REC dock gains a pause pill (reserve x=52 already specced round-1)
and the label becomes `REC 0:07 · 2.1 MB` — the live byte count is the
single highest-trust signal during a long take (CleanShot's file-size info).
Pause = disc turns `--accent`, label `PAUSED 0:07`, breathing instead of
pulsing. **Trash-with-confirm** (CleanShot) = hold ⌥ to reveal a trash
state on Stop. Countdown before start: a centered `3·2·1` chip in the
recording region, matches the self-timer norm. **P1** for pause+stats,
**P2** for countdown/bubble.

### 4.2 Finished recordings land in the album (P0)

Already true architecturally (`RecDone → open_card`, host.rs:596-600); the
stack just needs a recording-aware thumb: `▶` badge + duration chip
(`0:42` mono on `#0a0a0a` bottom-right), click opens the mp4 / preview.
CleanShot precedent: "Right after taking a screenshot **or recording a
video**, you'll see a small pop-up" — QAO holds both. [cleanshot.com/features]

### 4.3 Scroll-capture progress chrome (P0 — currently zero)

Quickshort: 450ms poll, 24-shot cap, 90s max, idle-stop after 3 identical
frames, `MIN_OVERLAP` rejects unrelated frames (scroll.rs:15-23) — all
silent until done.

Competitor chrome:
- **Snagit Panoramic**: a dedicated toolbar with **Start**, then the user
  scrolls, then **Stop** (toolbar can hide; taskbar icon revives it).
  [techsmith scrolling-capture tutorial]
- **CleanShot**: select region → **"Start Capture"** → scroll (or click
  Auto-Scroll) → **"Done"**; warns you to scroll slower if you outpace it.
  [scottwillsey.com walkthrough, setapp how-to]
- **Shottr**: auto-scrolls on release, "don't touch anything, Esc stops",
  and *shows a message when it can't scroll or hits max height*
  [shottr.cc/kb/scrollingcapture]. Max height is a *setting* (default 20k,
  up to 400k px) with a RAM warning — our `MAX_SHOTS`×height analog.

**Spec:** reuse the REC-dock window class: accent dot, `SCROLL · 3 shots ·
4,200 px` mono (shots = stitched frames, px = current height), `DONE` pill
+ `✕` cancel; Esc also stops. Failure states → toast copy keyed off
scroll.rs internals: no-match burst → "Can't match frames — scroll slower";
idle-stop → silent success (that's the normal end); `MAX_SHOTS` → "Max
length reached (24 screens)". Landing = tall thumb in the album (4.2,
letterbox). **P0** — it's the only capture mode with no feedback.

---

## 5. Pin windows

The ShareX pin spec is a complete, documented interaction set — steal it
wholesale [getsharex.com pin-to-screen]:

| Input | ShareX | Shottr | Ours today |
|---|---|---|---|
| wheel | scale 20–500% (steps configurable) | resize ✓ | — |
| Ctrl+wheel | opacity 10–100% | — (translucent pins drop their shadow) | — |
| middle-click | reset scale+opacity | — | — |
| double-click | collapse to mini preview | — | — |
| arrows / Shift | move 1px / 10px | — | — |
| right-click | close | — | drag+× only |
| keep-center-while-resizing | option | — | — |

**Spec (P1):** wheel = scale 0.25–4.0 around cursor-anchored center;
`Ctrl`/`⌘`+wheel = opacity 0.1–1.0 (DodoShot/StickyShot precedent, round-1);
middle-click = reset; translucent pins (`α<1`) drop the border+fringe —
Shottr's "semi-transparent pins lose their shadow" detail, which also fixes
the softbuffer-opaque-black-edges problem at low alpha. Multi-pin is already
a `Vec<Pin>` (host.rs); new pins should cascade-offset +24px so they don't
stack identically (pin.rs:47-58 centers *every* pin — a real overlap bug
for pin #2+). **P1, small diff.**
Pin-from-stack: right-click menu already lists "Pin to screen" (sim ctx) —
also allow **dragging a stack thumb straight up** to pin it (gesture = drop
on "nowhere" while holding ⇧?). Keep it menu-only if gesture feels
undiscoverable — P2.

---

## 6. Settings & tray

### 6.1 Settings search & conflict states (P1)

- 2596-line settings.rs with a sidebar already — add a **⌘F search field at
  the sidebar top** that filters rows by label+desc text (VS Code/macOS
  System Settings pattern; Raycast settings does not have search — this is
  ahead of the pack, not behind).
- **Hotkey conflict UI:** `global-hotkey` register failure is the silent
  killer — pot-desktop's fix is the model: register each hotkey
  *independently* (one bad binding shouldn't nuke all), probe on change,
  and show a row state: `⌃⇧S — already used by another app` in `--red`,
  offering 2–3 known-free alternates as clickable chips. Windows can't name
  the owner; macOS can't either for third-party chords — copy is honest:
  "held by another application". [pot-desktop PR #1292, PwrSnap#70,
  audiobud#193]
- Bare-key guard already exists (settings.rs:470) — extend the hint to
  suggest `PrintScreen`-class exceptions explicitly on Windows.

### 6.2 Tray: thumbnails in the Recent Captures submenu (P1 on macOS)

`muda 0.21` ships `IconMenuItem` — a menu item with a real image, on macOS
(NSMenuItem.image), Windows and GTK [docs.rs/muda IconMenuItem]. Our
submenu is filename-only text (tray.rs:33-44). Swap to `IconMenuItem` with
a ~22px thumb; macOS renders it natively. No competitor is verified to do
this (`[?]` for Shottr/CleanShot menus) — it's a cheap differentiator that
makes "Recent Captures" actually scannable.

### 6.3 First-run onboarding (P0 on macOS)

The screenpipe PR and a screen-recording scaffold template document the
trap precisely [github.com/screenpipe/screenpipe#5264,
xueshiqiao/macos-app-scaffold screen-recording README]:
- `CGPreflightScreenCaptureAccess` is **silent** — call it in onboarding to
  render status without triggering the system prompt; put the actual
  request behind a "Grant screen recording" button.
- Screen Recording grants only attach to the *next* process — show a
  `grantedPendingRelaunch` state ("Quit & Reopen required") rather than
  bouncing back to the permission screen (the Thuki issue shows the bug:
  stale-false preflight → onboarding loops).
- Sequence: card 1 = big keycap teaching `⌘⇧S` (the sim's intro pill is
  already this); card 2 = permission with a live green/red status dot +
  "Open System Settings" button; card 3 = "take your first shot" prompt.
  One-time, `~/.config/quickshort/onboarded` flag.

---

## 7. Cross-cutting

### 7.1 Keyboard-only end-to-end (P1)

Current gaps: stack unreachable by keyboard; lightbox/ctx menu keyboard-
opaque; toolbar tool keys exist but nothing moves *selection* focus. Full
chain should be: `⌘⇧S` → drag or `Enter` (full-screen select — macOS's
Enter=capture precedent [mh26782]) → tool keys → `⌘C` → stack focused via
tray/hotkey → arrows/`⌘C`/`⌘E`/`Del`/`Esc`. macshot's history overlay is
the reference implementation for the back half.

### 7.2 Reduced motion & accessibility (P1)

- macOS: `NSWorkspace.accessibilityDisplayShouldReduceMotion` → replace
  fly-to-corner with a 150ms fade-in on the stack + toast fallback.
  Windows: `SystemParametersInfo(SPI_GETCLIENTAREAANIMATION)`. GTK:
  `org.gnome.desktop.interface enable-animations`. The fly is *the* load-
  bearing animation; everything else (fan, hovers) is already subtle.
- **Focus/contrast**: all popups are custom raster surfaces — VoiceOver
  sees nothing. Minimum viable: on macOS set an `NSAccessibilityElement`
  label on each popup window ("3 captures — capture stack") so the count is
  announced; tray menu is already native (screen-reader-clean). `[?]` on
  exact winit hooks — may need objc2 glue like `widgets::round_window` does.
- Mono chips must respect `prefers-contrast` → bump `--muted` to `--ink`.

### 7.3 Localization & layouts (P1)

- Modifier glyphs per OS already flagged round-1; add: mono hint chips are
  fixed-width in the sim — translated labels ("Enregistrer", "Kopieren")
  will overflow a 272px toast; use `ellipsize()` (already exists,
  toast.rs:263) and let chips size to `Txt::width`.
- Non-QWERTY: single-char tool keys (`P`, `A`, `R`…) bind *characters*, not
  physical keys — on AZERTY `A`/`Q`/`W`/`Z` shift physically. `global-hotkey`
  reports physical codes on some platforms; safest is binding by
  `PhysicalKey` where the API offers it and rendering the *label* from the
  layout. [audiobud#193 — "Non-QWERTY layouts are a recurring blind spot"]

### 7.4 Fly-animation timing (P2 polish)

Sim uses `cubic-bezier(.25,.9,.3,1) · 600ms` — right family (overshooting
ease-out ≈ Apple's spring feel) but ~150ms long. macOS doesn't document
the thumbnail's curve `[?]`; SwiftUI's default `spring(duration: 0.35..0.4,
bounce ~0.2-0.3)` is the calibrated-feeling equivalent
[developer.apple.com spring(duration:bounce:)]. Also fade the accent frame
*during* flight (sim keeps the dashed accent border the whole way —
distracting once it's "become" a thumb; drop to the `#262626` rim at ~60%).

---

## 8. Spec deltas (implementation-ready)

```
STACK — beyond round-1 §4.3:
  overflow    : never drop thumbs; >5 collapse behind "N" chip; chip click
                pins the fan open; Esc/click-elsewhere re-collapses
  keyboard    : hover grants focus; ←→ move ring; Enter preview; Space
                quick-look; ⌘C copy; ⌘E editor; ⌘L share; ⌫ remove; ⌘Z
                undo-remove (session); Esc unfocus
  drag-out    : press ≥150ms + move ≥6px = file promise; macOS P0;
                drop nowhere = snap back; drop on Finder/Desktop = save
  thumbs      : cover-crop aspect∈[0.5,3], else letterbox on #050505;
                recording badge ▶ + "0:42" bottom-right; scroll shots = tall
  hide        : drag ≥40px toward screen edge → collapses to chip only
  restore     : on launch rehydrate ≤recent_max from save_dir, badge ✓;
                tray item "Reopen last capture"
  per-monitor : dock on capture's monitor (fix popup_monitor)

OVERLAY:
  modifiers   : during drag — ⌥ center-resize, ⇧ axis-lock, Space move;
                confirmed — ⇧+handle aspect-lock, arrows 1/10px nudge,
                ⌘arrows resize; R cycles 16:9/4:3/1:1/free
  size chip   : inside-center fallback when edges clip it (Flameshot
                xywh_position rule)
  cancel      : click-without-drag cancels; 1-2px accidental drag does NOT
                exit (lib.rs:1445-1454 — split the MIN_SELECT path)
  magnifier   : P2 — 96px circle, 8× NN, only when no tool armed

TOOLS:
  counter     : N; click = numbered disc auto-increment; options row
                [start n] [disc|square]; always-on-top z
  spotlight   : drag = dim all-but-rect; keys 1..9 = dim α while armed
  redact      : options-row chip on Rect = fill #000, label REDACT
  pixel|blur  : second chip on the X tool's row; blur = 3× box blur
  persistence : config [tool_style] table: tool → (size, color)

RECORD:
  dock        : "REC m:ss · N.N MB", pause pill at x=52 (accent, breathing),
                ⌥+Stop = trash-with-confirm; pre-countdown chip 3·2·1
  landing     : album thumb, ▶ badge + duration chip
  scroll dock : accent dot, "SCROLL · n shots · h px", DONE + ✕ pills;
                Esc stops; mismatch → toast "Can't match — scroll slower";
                MAX_SHOTS → "Max length reached (24 screens)"

PIN:
  wheel       : scale .25–4.0 anchored at cursor; ⌘/Ctrl+wheel = α .10–1.0;
                middle-click reset; α<1 drops border+fringe; new pins
                cascade +24px (pin.rs:47-58 centers all — fix)

SETTINGS/TRAY:
  search      : ⌘F field atop sidebar, filters rows live
  hotkey      : register each binding independently; conflict row state
                (--red "in use by another app" + 3 alternate chips)
  tray        : Recent Captures → IconMenuItem ~22px thumbs (muda 0.21)
  onboarding  : 3 cards — teach hotkey, silent TCC preflight + grant
                button + grantedPendingRelaunch state, first-shot prompt

A11Y/MOTION:
  fly         : 400ms spring bounce≈0.25; reduce-motion → 150ms fade
  chips       : measure via Txt::width, ellipsize long translations
```

---

## 9. Sources (new this round; round-1 sources not repeated)

- https://cleanshot.com/changelog — QAO 2.0 (Space=Quick Look, slide-down
  to hide, newest-shot indicator, close-all, post-upload interaction fixes),
  counter style/start-number/stay-on-top, Highlight rounded/ellipse, ⌥-copy
  without closing, file-size+sound info on video overlay, pinned-context
  Save/OCR, pin-border preference, Capture History (1 month)
- https://cleanshot.com/features — QAO multi-display/drag&drop/restore/
  auto-close config; recordings land in QAO; pause-resume/restart hotkeys
  via pie-menu listing + daveswift review
- https://www.imore.com/cleanshot-x-gains-new-keyboard-shortcuts-quick-access-overlay-more — QAO 4.1 keys, Move to trash
- https://github.com/sw33tLie/macshot/blob/v4.1.2/CHANGELOG.md — history
  overlay keyboard nav (arrows/Enter/⌘C/⌘E/Space/⌫, hover-synced
  selection), 11 configurable overlay shortcuts, Capture Last Area,
  per-tool lastUsedTool/strokeWidth persistence
- https://support.apple.com/guide/mac-help/take-a-screenshot-mh26782 —
  drag-phase modifiers (Space move / Shift axis / ⌥ center), thumbnail
  drag/swipe-right behaviors
- https://stackoverflow.com/questions/74140663 — `thumbnailExpiration`
  defaults key; ~5s default
- https://github.com/LnL7/nix-darwin/pull/965 — `last-selection` plist:
  macOS persists last rect natively
- https://osxdaily.com/2017/10/16/dismiss-ios-screenshot-preview-ios/ +
  ios.gadgethacks.com/…/0178452 + discussions.apple.com/thread/255831082 —
  iOS stack: swipe-left dismisses whole stack, tap = Markup-all, rapid
  shots auto-dismiss
- https://learn.microsoft.com/en-us/answers/questions/5674485… +
  edgedrop.app/blog/notification-click-vs-clipboard-for-snips — Snipping
  Tool toast→editor flow, complaints about notification-first, toast
  suppressed by Focus Assist
- https://shottr.cc/kb/scrollingcapture — auto-scroll, "don't touch
  anything", Esc stops, max-height setting (20k→400k px), failure message
- https://www.techsmith.com/learn/tutorials/snagit/scrolling-capture/ —
  Panoramic toolbar Start/Stop, hidden-toolbar recovery via taskbar icon
- https://scottwillsey.com/cleanshotx-scrolling-screenshots/ — CleanShot
  Start Capture → Auto-Scroll → Done flow, scroll-slower warning
- https://getsharex.com/docs/pin-to-screen.html — complete pin control
  spec (wheel scale 20–500%, Ctrl+wheel opacity, middle reset, dbl-click
  minimize, arrow nudge)
- https://www.daveswift.com/cleanshot-x/ — pause/restart/trash-with-confirm,
  camera bubble position/size/shape, Continuity Camera, typed resolution
- https://shottr.cc/ — scroll-resize pins, semi-transparent pins drop
  shadow, Spotlight 1..9 opacity keys, `then=copy,save,edit,pin` URL scheme
- https://flameshot.org/docs/dev/flameshot/classMagnifierWidget/ +
  PR #2219 + flameshot#3207 — magnifier round/square config, hidden during
  move tool, geometry readout inside-center option
- https://github.com/pot-app/pot-desktop/pull/1292 — hotkey registration:
  independent registration, stale-state fix, classify free/held/OS-error
- https://github.com/pwrdrvr/PwrSnap/issues/70 +
  https://github.com/jamditis/audiobud/issues/193 — conflict-probe
  register+isRegistered trick, "test your shortcut" row, non-QWERTY blind
  spot
- https://docs.rs/muda/latest/muda/struct.IconMenuItem.html — IconMenuItem
  (real images in menus, macOS/Windows/GTK)
- https://github.com/screenpipe/screenpipe/pull/5264 — silent preflight,
  request-behind-button, permission-recovery lifecycle
- https://github.com/xueshiqiao/macos-app-scaffold/…/screen-recording —
  ScreenCaptureKit relaunch invariant + `grantedPendingRelaunch` state
- https://github.com/quiet-node/thuki/issues/229 — stale-false
  CGPreflightScreenCaptureAccess after grant → onboarding loop bug
- https://getstream.io/chat/docs/sdk/react/guides/sending-messages-with-pending-uploads.md —
  pending-upload send semantics rationale
- https://slack.com/help/articles/201330736 — composer chips: per-file
  preview, reorder, ≤10 files
- https://developer.apple.com/documentation/swiftui/animation/spring(duration:bounce:blendduration:) —
  spring defaults for flight timing
