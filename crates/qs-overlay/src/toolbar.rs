//! Selection toolbar: circular action buttons docked to whichever selection
//! edge has room — bottom -> top -> right -> left, falling back inside the
//! selection's bottom-right corner. Rendered straight into the softbuffer
//! surface via SDF primitives at physical resolution (crisp on HiDPI).

use crate::annotate::PALETTE;
use crate::sdf::{ellipse_sdf, fill_cov, rrect_sdf, seg_sdf, stroke_cov, Canvas};
use qs_core::{Point, Rect};

/// Terminal actions — leave the overlay with a result.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Save,
    Copy,
    Ocr,
    /// Publish the capture through an SSH tunnel and copy its link
    Share,
    /// Repeated captures of this rect while the user scrolls, stitched
    /// into one tall image
    Scroll,
}

/// Drawing tools — stay in the overlay and annotate the canvas.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    /// Pick, move and delete existing annotations
    Select,
    Pen,
    Arrow,
    Rect,
    Ellipse,
    Marker,
    Text,
    Pixelate,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    Tool(Tool),
    Act(Action),
    Undo,
    Redo,
    /// Stroke size level 0..=2 — options row, shown while a tool is armed
    Size(u8),
    /// Palette index — options row
    Color(usize),
    /// Bold toggle — options row, only while the text tool is armed
    Bold,
    /// Font size preset index into annotate::TEXT_SIZES — text tool only
    FontSize(u8),
}

const MAIN: &[Item] = &[
    Item::Tool(Tool::Select),
    Item::Tool(Tool::Pen),
    Item::Tool(Tool::Arrow),
    Item::Tool(Tool::Rect),
    Item::Tool(Tool::Ellipse),
    Item::Tool(Tool::Marker),
    Item::Tool(Tool::Text),
    Item::Tool(Tool::Pixelate),
    Item::Undo,
    Item::Redo,
    Item::Act(Action::Save),
    Item::Act(Action::Copy),
    Item::Act(Action::Scroll),
    Item::Act(Action::Ocr),
    Item::Act(Action::Share),
];

const OPTS: &[Item] = &[
    Item::Size(0),
    Item::Size(1),
    Item::Size(2),
    Item::Color(0),
    Item::Color(1),
    Item::Color(2),
    Item::Color(3),
    Item::Color(4),
    Item::Color(5),
    Item::Color(6),
    Item::Color(7),
];

const R: f64 = 14.0; // button radius, logical px
const GAP: f64 = 8.0; // between buttons
const PAD: f64 = 6.0; // toolbar edge padding
const EDGE_GAP: f64 = 4.0; // between selection edge and toolbar
const STROKE: f64 = 1.8; // icon stroke width, logical px

const BG: u32 = 0x2C2C2E;
const BG_HOVER: u32 = 0x48484D;
const FG: u32 = 0xFFFFFF;
const SHADOW: u32 = 0x000000;

pub struct Toolbar {
    /// bounding box in virtual logical coords
    pub rect: Rect,
    /// button hit rects in logical coords — main row then options row
    pub buttons: Vec<(Item, Rect)>,
    pub hover: Option<usize>,
    /// currently armed drawing tool (highlighted + shows the options row)
    pub tool: Option<Tool>,
    /// selected size level / palette index for the next stroke
    pub size: u8,
    pub color: usize,
    /// bold state for the text tool
    pub bold: bool,
    /// selected index into annotate::TEXT_SIZES for the text tool
    pub font_size: u8,
    /// whether Undo/Redo have something to pop (dims the buttons otherwise)
    pub can_undo: bool,
    pub has_redo: bool,
    /// active-chip color — the configured accent
    pub accent: u32,
}

impl Toolbar {
    /// `mr` = logical rect of the monitor holding the selection.
    /// `opts` shows the second row (size + color) when a tool is armed.
    pub fn new(sel: Rect, mr: Rect, opts: bool) -> Self {
        let mut tb = Self {
            rect: Rect::default(),
            buttons: Vec::new(),
            hover: None,
            tool: None,
            size: 1,
            color: 0,
            bold: false,
            font_size: crate::annotate::TEXT_SIZE_DEFAULT as u8,
            can_undo: false,
            has_redo: false,
            accent: crate::ACCENT,
        };
        tb.layout(sel, mr, opts);
        tb
    }

    /// Recompute rect + button slots — call when the options row toggles.
    pub fn layout(&mut self, sel: Rect, mr: Rect, opts: bool) {
        let d = R * 2.0;
        let n = MAIN.len() as f64;
        // the text tool swaps stroke sizes for real font sizes + a Bold
        // toggle; colors stay shared
        let opts_row: Vec<Item> = if self.tool == Some(Tool::Text) {
            (0..crate::annotate::TEXT_SIZES.len() as u8)
                .map(Item::FontSize)
                .chain([Item::Bold])
                .chain((0..PALETTE.len()).map(Item::Color))
                .collect()
        } else {
            OPTS.to_vec()
        };
        let on = opts_row.len() as f64;
        let rows = if opts { 2.0 } else { 1.0 };
        // second row aligns under the main row, slightly narrower
        let (hw, hh) = (
            PAD * 2.0 + n.max(on) * d + (n.max(on) - 1.0) * GAP,
            PAD * 2.0 + rows * d + (rows - 1.0) * GAP * 0.5,
        );
        let (vw, vh) = (hh, hw);
        let (sx0, sy0) = (sel.x as f64, sel.y as f64);
        let (sx1, sy1) = (sel.right() as f64, sel.bottom() as f64);
        let (mx0, my0) = (mr.x as f64, mr.y as f64);
        let (mx1, my1) = (mr.right() as f64, mr.bottom() as f64);
        let clamp = |v: f64, lo: f64, hi: f64| if lo <= hi { v.clamp(lo, hi) } else { hi };

        // (x, y, vertical, fits?)
        let candidates = [
            (
                clamp(sx1 - hw, mx0 + 2.0, mx1 - hw - 2.0),
                sy1 + EDGE_GAP,
                false,
                sy1 + EDGE_GAP + hh <= my1,
            ),
            (
                clamp(sx1 - hw, mx0 + 2.0, mx1 - hw - 2.0),
                sy0 - EDGE_GAP - hh,
                false,
                sy0 - EDGE_GAP - hh >= my0,
            ),
            (
                sx1 + EDGE_GAP,
                clamp(sy1 - vh, my0 + 2.0, my1 - vh - 2.0),
                true,
                sx1 + EDGE_GAP + vw <= mx1,
            ),
            (
                sx0 - EDGE_GAP - vw,
                clamp(sy1 - vh, my0 + 2.0, my1 - vh - 2.0),
                true,
                sx0 - EDGE_GAP - vw >= mx0,
            ),
            // inside bottom-right of the selection — always the fallback
            (
                clamp(sx1 - hw - EDGE_GAP, mx0 + 2.0, mx1 - hw - 2.0),
                clamp(sy1 - hh - EDGE_GAP, my0 + 2.0, my1 - hh - 2.0),
                false,
                true,
            ),
        ];

        let (x, y, vertical, _) = candidates.iter().find(|c| c.3).copied().unwrap();
        self.rect = Rect::new(
            x.round() as i32,
            y.round() as i32,
            (if vertical { vw } else { hw }).round() as u32,
            (if vertical { vh } else { hh }).round() as u32,
        );
        let row_step = d + GAP * 0.5;
        let place = |items: &[Item], second: bool, buttons: &mut Vec<(Item, Rect)>| {
            for (i, it) in items.iter().enumerate() {
                let off = PAD + i as f64 * (d + GAP);
                let off2 = PAD + if second { row_step } else { 0.0 };
                let r = if vertical {
                    Rect::new(
                        self.rect.x + off2.round() as i32,
                        self.rect.y + off.round() as i32,
                        d as u32,
                        d as u32,
                    )
                } else {
                    Rect::new(
                        self.rect.x + off.round() as i32,
                        self.rect.y + off2.round() as i32,
                        d as u32,
                        d as u32,
                    )
                };
                buttons.push((*it, r));
            }
        };
        self.buttons.clear();
        place(MAIN, false, &mut self.buttons);
        if opts {
            place(&opts_row, true, &mut self.buttons);
        }
    }

    /// Index of the button under `p`, or `None`.
    pub fn hit(&self, p: Point) -> Option<usize> {
        self.buttons
            .iter()
            .position(|(_, r)| p.x >= r.x && p.x < r.right() && p.y >= r.y && p.y < r.bottom())
    }

    /// `p` inside the toolbar box (incl. padding) — clicks there should not
    /// start a new selection.
    pub fn contains(&self, p: Point) -> bool {
        let r = &self.rect;
        p.x >= r.x && p.x < r.right() && p.y >= r.y && p.y < r.bottom()
    }

    /// Paint buttons into the physical surface. `vrect` = this window's monitor
    /// rect in virtual coords; parts outside this monitor clip naturally.
    pub fn paint(&self, buf: &mut [u32], sw: usize, sh: usize, vrect: Rect, scale: f64) {
        let mut cv = Canvas {
            buf,
            w: sw,
            h: sh,
            scale,
        };
        for (i, (item, r)) in self.buttons.iter().enumerate() {
            let cx = (r.x as f64 + r.w as f64 / 2.0 - vrect.x as f64) * scale;
            let cy = (r.y as f64 + r.h as f64 / 2.0 - vrect.y as f64) * scale;
            let active = matches!(item, Item::Tool(t) if self.tool == Some(*t))
                || matches!(item, Item::Size(s) if self.size == *s)
                || matches!(item, Item::Color(c) if self.color == *c)
                || matches!(item, Item::FontSize(s) if self.font_size == *s)
                || matches!(item, Item::Bold) && self.bold;
            let enabled = match item {
                Item::Undo => self.can_undo,
                Item::Redo => self.has_redo,
                _ => true,
            };
            paint_button(
                &mut cv,
                cx,
                cy,
                R * scale,
                self.hover == Some(i),
                active,
                enabled,
                *item,
                self.accent,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_button(
    cv: &mut Canvas,
    cx: f64,
    cy: f64,
    rad: f64,
    hovered: bool,
    active: bool,
    enabled: bool,
    item: Item,
    accent: u32,
) {
    let scale = cv.scale;
    // bbox covers button + soft shadow falloff
    let pad = (STROKE * scale).ceil() as i32 + (4.0 * scale).ceil() as i32 + 2;
    let r = rad.ceil() as i32 + pad;
    let bg = if active {
        accent
    } else if hovered {
        BG_HOVER
    } else {
        BG
    };
    let fg_a = if enabled { 1.0 } else { 0.35 };
    for y in (cy as i32 - r)..=(cy as i32 + r) {
        for x in (cx as i32 - r)..=(cx as i32 + r) {
            let px = x as f64 + 0.5 - cx;
            let py = y as f64 + 0.5 - cy;
            // soft shadow: disc shifted down, alpha falls off over ~4 phys px
            let sd = px.hypot(py - 1.5 * scale) - rad;
            if sd > 0.0 {
                let a = 0.24 * (1.0 - sd / (4.0 * scale)).clamp(0.0, 1.0);
                if a > 0.0 {
                    cv.blend(x, y, SHADOW, a);
                }
            }
            // disc + hairline rim
            let d = px.hypot(py) - rad;
            let a = (0.5 - d).clamp(0.0, 1.0);
            if a > 0.0 {
                cv.blend(x, y, bg, a * 0.9);
                let rim = (0.5 - d.abs()).clamp(0.0, 1.0);
                if rim > 0.0 {
                    cv.blend(x, y, FG, rim * 0.12);
                }
            }
            // icon — work in logical units, sdf * scale -> physical coverage
            let (lx, ly) = (px / scale, py / scale);
            draw_icon(cv, x, y, lx, ly, scale, bg, fg_a, item);
        }
    }
}

/// Icon stroke/fill for one toolbar item at logical point (lx, ly).
#[allow(clippy::too_many_arguments)]
fn draw_icon(
    cv: &mut Canvas,
    x: i32,
    y: i32,
    lx: f64,
    ly: f64,
    scale: f64,
    bg: u32,
    fg_a: f64,
    item: Item,
) {
    let stroke = |d: f64, w: f64| stroke_cov(d, w, scale);
    match item {
        Item::Tool(Tool::Select) => {
            // pointer arrow: outline of the classic up-left cursor
            let pts = [
                (-4.2, -5.6),
                (-4.2, 4.0),
                (-1.4, 1.4),
                (0.8, 5.0),
                (2.9, 4.2),
                (0.7, -0.2),
                (4.6, -0.2),
                (-4.2, -5.6),
            ];
            let mut d = f64::INFINITY;
            for s in pts.windows(2) {
                d = d.min(seg_sdf(lx, ly, s[0].0, s[0].1, s[1].0, s[1].1));
            }
            let a = stroke(d, 1.6) * fg_a;
            if a > 0.0 {
                cv.blend(x, y, FG, a);
            }
        }
        Item::Tool(Tool::Pen) => {
            // pencil: body + pointed nib
            let d =
                seg_sdf(lx, ly, -4.6, 4.6, 1.4, -1.4).min(seg_sdf(lx, ly, 1.4, -1.4, 4.6, -4.4));
            let a = stroke(d, 2.4) * fg_a;
            if a > 0.0 {
                cv.blend(x, y, FG, a);
            }
        }
        Item::Tool(Tool::Arrow) => {
            // ↗ shaft + two wings
            let d = seg_sdf(lx, ly, -5.2, 5.2, 3.6, -3.6)
                .min(seg_sdf(lx, ly, 3.6, -3.6, -0.6, -3.6))
                .min(seg_sdf(lx, ly, 3.6, -3.6, 3.6, 0.6));
            let a = stroke(d, STROKE) * fg_a;
            if a > 0.0 {
                cv.blend(x, y, FG, a);
            }
        }
        Item::Tool(Tool::Rect) => {
            let a = stroke(rrect_sdf(lx, ly, 0.0, 0.0, 4.8, 3.8, 0.9), STROKE) * fg_a;
            if a > 0.0 {
                cv.blend(x, y, FG, a);
            }
        }
        Item::Tool(Tool::Ellipse) => {
            let a = stroke(ellipse_sdf(lx, ly, 0.0, 0.0, 5.0, 3.8), STROKE) * fg_a;
            if a > 0.0 {
                cv.blend(x, y, FG, a);
            }
        }
        Item::Tool(Tool::Marker) => {
            // highlighter: wide body + thin nib
            let d = seg_sdf(lx, ly, -4.8, 4.4, 1.6, -2.0);
            let a = stroke(d, 3.4) * fg_a;
            if a > 0.0 {
                cv.blend(x, y, FG, a);
            }
            let d2 = seg_sdf(lx, ly, 1.6, -2.0, 4.8, -5.2);
            let a = stroke(d2, STROKE) * fg_a;
            if a > 0.0 {
                cv.blend(x, y, FG, a);
            }
        }
        Item::Tool(Tool::Text) | Item::Bold | Item::FontSize(_) => {
            // real font glyphs — rasterized once into coverage grids
            let (lbl, b) = match item {
                Item::Bold => ("B", true),
                Item::FontSize(i) => (crate::annotate::TEXT_SIZES[i as usize].1, false),
                _ => ("T", false),
            };
            let m = crate::annotate::label_mask(lbl, b);
            let n = crate::annotate::ICON_MASK_N;
            let gx = ((lx + 8.0) / 16.0 * n as f64) as i32;
            let gy = ((ly + 8.0) / 16.0 * n as f64) as i32;
            let a = if (0..n as i32).contains(&gx) && (0..n as i32).contains(&gy) {
                m[gy as usize * n + gx as usize] as f64 * fg_a
            } else {
                0.0
            };
            if a > 0.0 {
                cv.blend(x, y, FG, a);
            }
        }
        Item::Tool(Tool::Pixelate) => {
            // 2x2 checkerboard of filled squares
            let d = rrect_sdf(lx, ly, -2.9, -2.9, 1.7, 1.7, 0.4)
                .min(rrect_sdf(lx, ly, 2.9, -2.9, 1.7, 1.7, 0.4))
                .min(rrect_sdf(lx, ly, -2.9, 2.9, 1.7, 1.7, 0.4))
                .min(rrect_sdf(lx, ly, 2.9, 2.9, 1.7, 1.7, 0.4));
            let a = fill_cov(d, scale) * fg_a;
            if a > 0.0 {
                cv.blend(x, y, FG, a);
            }
        }
        Item::Undo | Item::Redo => {
            // ↩/↪ hook: chevron tip + shaft + U-turn arc returning left,
            // mirrored for redo
            let m = if item == Item::Undo { 1.0 } else { -1.0 };
            let tip = (-4.6 * m, -0.6);
            let mut d = seg_sdf(lx, ly, tip.0, tip.1, tip.0 + 3.0 * m, tip.1 - 3.0)
                .min(seg_sdf(lx, ly, tip.0, tip.1, tip.0 + 3.0 * m, tip.1 + 3.0))
                .min(seg_sdf(lx, ly, tip.0, tip.1, 1.8 * m, tip.1));
            let (ccx, ccy, rr) = (1.8, 2.6, 3.2);
            let mut prev = (m * ccx, ccy - rr);
            for i in 1..=12 {
                let a = (-90.0 + 180.0 * i as f64 / 12.0).to_radians();
                let p = (m * (ccx + rr * a.cos()), ccy + rr * a.sin());
                d = d.min(seg_sdf(lx, ly, prev.0, prev.1, p.0, p.1));
                prev = p;
            }
            d = d.min(seg_sdf(lx, ly, prev.0, prev.1, prev.0 - 2.0 * m, prev.1));
            let a = stroke(d, STROKE) * fg_a;
            if a > 0.0 {
                cv.blend(x, y, FG, a);
            }
        }
        Item::Size(s) => {
            // filled disc, radius grows with the level
            let r = [1.8, 3.0, 4.6][s as usize];
            let d = lx.hypot(ly) - r;
            let a = fill_cov(d, scale) * fg_a;
            if a > 0.0 {
                cv.blend(x, y, FG, a);
            }
        }
        Item::Color(c) => {
            // filled swatch disc in the palette color
            let d = lx.hypot(ly) - 6.0;
            let a = fill_cov(d, scale) * fg_a;
            if a > 0.0 {
                cv.blend(x, y, PALETTE[c], a);
            }
        }
        Item::Act(Action::Copy) => {
            let d_back = rrect_sdf(lx, ly, -3.5, -3.5, 4.5, 4.5, 1.4);
            let d_front = rrect_sdf(lx, ly, 3.5, 3.5, 4.5, 4.5, 1.4);
            let fill = fill_cov(d_front, scale);
            if fill > 0.0 {
                cv.blend(x, y, bg, fill); // occlude back square
            }
            let a = stroke(d_back, STROKE).max(stroke(d_front, STROKE)) * fg_a;
            if a > 0.0 {
                cv.blend(x, y, FG, a);
            }
        }
        Item::Act(Action::Ocr) => {
            // paragraph: left-aligned lines, last one shorter
            let d = seg_sdf(lx, ly, -5.75, -4.5, 4.5, -4.5)
                .min(seg_sdf(lx, ly, -5.75, 0.0, 5.75, 0.0))
                .min(seg_sdf(lx, ly, -5.75, 4.5, 1.5, 4.5));
            let a = stroke(d, STROKE) * fg_a;
            if a > 0.0 {
                cv.blend(x, y, FG, a);
            }
        }
        Item::Act(Action::Share) => {
            // share: open-top box + arrow rising out of it
            let d = seg_sdf(lx, ly, -5.0, -1.0, -5.0, 5.5)
                .min(seg_sdf(lx, ly, -5.0, 5.5, 5.0, 5.5))
                .min(seg_sdf(lx, ly, 5.0, 5.5, 5.0, -1.0))
                .min(seg_sdf(lx, ly, 0.0, -6.5, 0.0, 0.5))
                .min(seg_sdf(lx, ly, 0.0, -6.5, -3.0, -3.5))
                .min(seg_sdf(lx, ly, 0.0, -6.5, 3.0, -3.5));
            let a = stroke(d, STROKE) * fg_a;
            if a > 0.0 {
                cv.blend(x, y, FG, a);
            }
        }
        Item::Act(Action::Scroll) => {
            // tall page outline + down chevron inside
            let d_page = rrect_sdf(lx, ly, 0.0, 0.0, 4.0, 5.5, 1.2);
            let d_arrow = seg_sdf(lx, ly, 0.0, -2.5, 0.0, 2.5)
                .min(seg_sdf(lx, ly, -2.2, 0.3, 0.0, 2.5))
                .min(seg_sdf(lx, ly, 2.2, 0.3, 0.0, 2.5));
            let a = stroke(d_page, STROKE).max(stroke(d_arrow, STROKE)) * fg_a;
            if a > 0.0 {
                cv.blend(x, y, FG, a);
            }
        }
        Item::Act(Action::Save) => {
            // tray: U shape open at top; arrow dips into it
            let d = seg_sdf(lx, ly, -5.75, -0.5, -5.75, 5.5)
                .min(seg_sdf(lx, ly, -5.75, 5.5, 5.75, 5.5))
                .min(seg_sdf(lx, ly, 5.75, 5.5, 5.75, -0.5))
                .min(seg_sdf(lx, ly, 0.0, -6.5, 0.0, 1.5))
                .min(seg_sdf(lx, ly, -3.2, -1.5, 0.0, 1.5))
                .min(seg_sdf(lx, ly, 3.2, -1.5, 0.0, 1.5));
            let a = stroke(d, STROKE) * fg_a;
            if a > 0.0 {
                cv.blend(x, y, FG, a);
            }
        }
    }
}
