//! Caption layout and burn-in.
//!
//! [`overlay`] lays out the caption showing on a track at a time with the track's
//! [`CaptionStyle`] (size relative to a 1080-line frame, colour, background box, outline,
//! alignment, anchor and margin; WebVTT `line:N%` and `align:` cue settings override the style)
//! and rasterises it into a small premultiplied linear-light RGBA [`Overlay`] positioned in the
//! frame. Renderers composite overlays over the finished picture.

use filmcraft_color::srgb_to_linear;
use filmcraft_project::{Caption, CaptionAlign, CaptionAnchor, CaptionStyle, CaptionTrack, Sequence};
use filmcraft_time::Tick;

use filmcraft_text::{ParagraphStyle, TextStyle, layout, render};

/// A rendered caption: premultiplied linear-light RGBA f32 pixels at `(x, y)` in the frame.
#[derive(Clone, Debug, PartialEq)]
pub struct Overlay {
    pub x: i32,
    pub y: i32,
    pub w: usize,
    pub h: usize,
    pub px: Vec<f32>,
}

impl Overlay {
    /// Composite over a premultiplied RGBA f32 canvas of `cw`×`ch` pixels.
    pub fn composite_onto(&self, canvas: &mut [f32], cw: usize, ch: usize) {
        for y in 0..self.h {
            let cy = self.y + y as i32;
            if cy < 0 || cy >= ch as i32 {
                continue;
            }
            for x in 0..self.w {
                let cx = self.x + x as i32;
                if cx < 0 || cx >= cw as i32 {
                    continue;
                }
                let s = &self.px[(y * self.w + x) * 4..(y * self.w + x) * 4 + 4];
                if s[3] <= 0.0 {
                    continue;
                }
                let i = (cy as usize * cw + cx as usize) * 4;
                let k = 1.0 - s[3];
                for c in 0..4 {
                    canvas[i + c] = s[c] + canvas[i + c] * k;
                }
            }
        }
    }
}

/// The caption face: the track's font family (Inter when empty or not installed) and weight
/// (`SemiBold` when the style is empty, the nearest face when the family lacks the named one).
fn caption_style(font: &str, style: &str, px: f32) -> TextStyle {
    let family = if font.trim().is_empty() { filmcraft_text::fonts::DEFAULT_FAMILY } else { font.trim() };
    let style = if style.trim().is_empty() { "SemiBold" } else { style.trim() };
    TextStyle { family: family.into(), style: style.into(), size: px, ..Default::default() }
}

fn lin(c: [u8; 4]) -> [f32; 4] {
    let a = c[3] as f32 / 255.0;
    [srgb_to_linear(c[0] as f32 / 255.0) * a, srgb_to_linear(c[1] as f32 / 255.0) * a, srgb_to_linear(c[2] as f32 / 255.0) * a, a]
}

/// Word-wrap one line to `max_w` pixels.
fn wrap(line: &str, ts: &TextStyle, max_w: f32) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in line.split(' ') {
        let cand = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
        if !cur.is_empty() && filmcraft_text::measure(&cand, ts) > max_w {
            out.push(std::mem::take(&mut cur));
            cur = word.to_string();
        } else {
            cur = cand;
        }
    }
    out.push(cur);
    out
}

/// Placement overrides from WebVTT cue settings.
fn vtt_overrides(settings: &str) -> (Option<f32>, Option<CaptionAlign>) {
    let mut line = None;
    let mut align = None;
    for kv in settings.split_whitespace() {
        let Some((k, v)) = kv.split_once(':') else { continue };
        match k {
            "line" => {
                let v = v.split(',').next().unwrap_or("");
                if let Some(p) = v.strip_suffix('%').and_then(|p| p.parse::<f32>().ok()) {
                    line = Some((p / 100.0).clamp(0.0, 1.0));
                }
            }
            "align" => {
                align = match v {
                    "start" | "left" => Some(CaptionAlign::Left),
                    "end" | "right" => Some(CaptionAlign::Right),
                    "center" | "middle" => Some(CaptionAlign::Center),
                    _ => None,
                }
            }
            _ => {}
        }
    }
    (line, align)
}

/// Lay out and rasterise one caption for a `w`×`h` frame.
pub fn render_caption(c: &Caption, style: &CaptionStyle, w: usize, h: usize) -> Option<Overlay> {
    if w == 0 || h == 0 {
        return None;
    }
    let scale = h as f32 / 1080.0;
    let px = (style.size * scale).max(4.0);
    let max_w = w as f32 * 0.9;
    let ts = caption_style(&style.font, &style.style, px);
    let mut lines: Vec<String> = Vec::new();
    // (speaker names are metadata; like Premiere, they are not burned in)
    for l in c.plain_lines() {
        lines.extend(wrap(l.trim(), &ts, max_w));
    }
    lines.retain(|l| !l.is_empty());
    if lines.is_empty() {
        return None;
    }
    let vm = filmcraft_text::fonts::face(filmcraft_text::resolve(&ts.family, &ts.style).face).metrics(px);
    let (asc, desc) = (vm.ascent, vm.descent);
    let lh = px * style.line_spacing.max(0.8);
    let pad_x = (px * 0.3).round();
    let block_h = lh * lines.len() as f32;
    let (line_override, align_override) = vtt_overrides(&c.settings);
    let align = align_override.unwrap_or(style.align);
    let margin = style.margin.clamp(0.0, 0.45) * h as f32;
    let anchor_top = match (line_override, style.anchor) {
        (Some(f), _) => (f * h as f32).min(h as f32 - block_h),
        (None, CaptionAnchor::Top) => margin,
        (None, CaptionAnchor::Middle) => (h as f32 - block_h) / 2.0,
        (None, CaptionAnchor::Bottom) => h as f32 - margin - block_h,
    };
    let outline = (style.outline * scale).max(0.0);
    // the text keeps the padding the outline needs; the shadow may reach further, so it grows the
    // overlay without pushing the glyphs around
    let edge = outline.ceil() + 1.0;
    let (sdx, sdy, ssoft) = if style.shadow {
        ((style.shadow_offset_x * scale).round() as i32, (style.shadow_offset_y * scale).round() as i32, (style.shadow_softness * scale).max(0.0))
    } else {
        (0, 0, 0.0)
    };
    let reach = if style.shadow { ssoft + sdx.unsigned_abs().max(sdy.unsigned_abs()) as f32 } else { 0.0 };
    let grow = edge.max(reach.ceil() + 1.0);
    // the style's nudge moves the whole block; the drawn box (padding and outline included) stays in
    // the frame, so the overlay never covers pixels outside it
    let top = (anchor_top + style.offset_y * h as f32).clamp(edge, (h as f32 - block_h - edge).max(edge)).round();
    let side = w as f32 * 0.05;
    let widths: Vec<f32> = lines.iter().map(|l| filmcraft_text::measure(l, &ts)).collect();
    let xs: Vec<f32> = widths
        .iter()
        .map(|&lw| {
            let x = match align {
                CaptionAlign::Left => side + pad_x,
                CaptionAlign::Right => w as f32 - side - pad_x - lw,
                CaptionAlign::Center => (w as f32 - lw) / 2.0,
            };
            x + style.offset_x * w as f32
        })
        .collect();
    let inset = pad_x + edge;
    let xs: Vec<f32> = xs.into_iter().zip(&widths).map(|(x, &lw)| x.clamp(inset, (w as f32 - lw - inset).max(inset))).collect();
    let bx0 = xs.iter().zip(&widths).map(|(x, _)| x - pad_x - grow).fold(f32::MAX, f32::min).floor();
    let bx1 = xs.iter().zip(&widths).map(|(x, lw)| x + lw + pad_x + grow).fold(f32::MIN, f32::max).ceil();
    let by0 = (top - grow).floor();
    let by1 = (top + block_h + grow).ceil();
    let (ox, oy) = (bx0 as i32, by0 as i32);
    let ow = (bx1 - bx0).max(1.0) as usize;
    let oh = (by1 - by0).max(1.0) as usize;
    let mut cover = vec![0.0f32; ow * oh];
    let mut bg = vec![0.0f32; ow * oh];
    for (i, line) in lines.iter().enumerate() {
        let lt = top + lh * i as f32;
        if style.background && style.background_color[3] > 0 {
            let (x0, x1) = (xs[i] - pad_x, xs[i] + widths[i] + pad_x);
            let (y0, y1) = (lt, lt + lh);
            for y in (y0.floor() as i32).max(oy)..(y1.ceil() as i32).min(oy + oh as i32) {
                let fy = ((y as f32 + 1.0).min(y1) - (y as f32).max(y0)).clamp(0.0, 1.0);
                for x in (x0.floor() as i32).max(ox)..(x1.ceil() as i32).min(ox + ow as i32) {
                    let fx = ((x as f32 + 1.0).min(x1) - (x as f32).max(x0)).clamp(0.0, 1.0);
                    let j = (y - oy) as usize * ow + (x - ox) as usize;
                    bg[j] = (bg[j] + fx * fy).min(1.0);
                }
            }
        }
        let baseline = lt + (lh - (asc + desc)) / 2.0 + asc;
        let l = layout(line, &ts, &ParagraphStyle::default());
        let mut m = filmcraft_text::Mask { w: ow, h: oh, a: std::mem::take(&mut cover) };
        render::draw(&l, &render::at(xs[i], baseline.round()), &mut m, (ox, oy));
        cover = m.a;
    }
    let stroke = if outline > 0.0 { dilate(&cover, ow, oh, outline) } else { Vec::new() };
    let shadow = if style.shadow { blur_shift(&cover, ow, oh, sdx, sdy, ssoft) } else { Vec::new() };
    let (tc, bc, oc) = (lin(style.color), lin(style.background_color), lin(style.outline_color));
    let sc = lin(style.shadow_color);
    let mut out = vec![0.0f32; ow * oh * 4];
    for j in 0..ow * oh {
        let mut p = [0.0f32; 4];
        let over = |p: &mut [f32; 4], c: [f32; 4], a: f32| {
            if a > 0.0 {
                for k in 0..4 {
                    p[k] = c[k] * a + p[k] * (1.0 - c[3] * a);
                }
            }
        };
        over(&mut p, bc, bg[j]);
        if !shadow.is_empty() {
            over(&mut p, sc, shadow[j]);
        }
        if !stroke.is_empty() {
            over(&mut p, oc, stroke[j]);
        }
        over(&mut p, tc, cover[j]);
        out[j * 4..j * 4 + 4].copy_from_slice(&p);
    }
    Some(Overlay { x: ox, y: oy, w: ow, h: oh, px: out })
}

/// Blur a coverage mask and shift it by (`dx`, `dy`) — the shadow of the glyphs.
fn blur_shift(m: &[f32], w: usize, h: usize, dx: i32, dy: i32, soft: f32) -> Vec<f32> {
    let r = soft.round() as i32;
    let src = if r > 0 { box_blur(m, w, h, r) } else { m.to_vec() };
    let mut out = vec![0.0f32; w * h];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let (sx, sy) = (x - dx, y - dy);
            if sx >= 0 && sy >= 0 && sx < w as i32 && sy < h as i32 {
                out[y as usize * w + x as usize] = src[sy as usize * w + sx as usize];
            }
        }
    }
    out
}

/// Separable box blur with clamped edges — cheap, and indistinguishable from a soft shadow at this size.
fn box_blur(m: &[f32], w: usize, h: usize, r: i32) -> Vec<f32> {
    let (mut tmp, mut out) = (vec![0.0f32; w * h], vec![0.0f32; w * h]);
    let norm = (2 * r + 1) as f32;
    for y in 0..h {
        for x in 0..w as i32 {
            let mut s = 0.0;
            for d in -r..=r {
                s += m[y * w + (x + d).clamp(0, w as i32 - 1) as usize];
            }
            tmp[y * w + x as usize] = s / norm;
        }
    }
    for y in 0..h as i32 {
        for x in 0..w {
            let mut s = 0.0;
            for d in -r..=r {
                s += tmp[(y + d).clamp(0, h as i32 - 1) as usize * w + x];
            }
            out[y as usize * w + x] = s / norm;
        }
    }
    out
}

/// Max-filter a coverage mask with a disc of radius `r`.
fn dilate(m: &[f32], w: usize, h: usize, r: f32) -> Vec<f32> {
    let ri = r.ceil() as i32;
    let mut offs = Vec::new();
    for dy in -ri..=ri {
        for dx in -ri..=ri {
            let d = ((dx * dx + dy * dy) as f32).sqrt();
            if d <= r + 0.5 {
                offs.push((dx, dy, (r + 0.5 - d).clamp(0.0, 1.0)));
            }
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let mut v = 0.0f32;
            for &(dx, dy, wgt) in &offs {
                let (sx, sy) = (x + dx, y + dy);
                if sx >= 0 && sy >= 0 && sx < w as i32 && sy < h as i32 {
                    v = v.max(m[sy as usize * w + sx as usize] * wgt);
                }
            }
            out[y as usize * w + x as usize] = v;
        }
    }
    out
}

/// The overlay for a caption track at timeline time `t` (None when nothing shows).
pub fn track_overlay(track: &CaptionTrack, t: Tick, w: usize, h: usize) -> Option<Overlay> {
    if !track.enabled {
        return None;
    }
    let c = track.caption_at(t)?;
    render_caption(c, &track.style, w, h)
}

/// Overlays of every visible caption track of a sequence at `t`, bottom track first.
pub fn sequence_overlays(seq: &Sequence, t: Tick, w: usize, h: usize) -> Vec<Overlay> {
    seq.caption_tracks.iter().rev().filter_map(|tr| track_overlay(tr, t, w, h)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_project::{CaptionFormat, ClipId, TrackId};

    fn cap(text: &str) -> Caption {
        Caption { id: ClipId(1), start: Tick(0), duration: Tick(100), text: text.into(), speaker: None, cue_id: None, settings: String::new() }
    }

    #[test]
    fn bottom_centre_with_box() {
        let st = CaptionStyle::default();
        let o = render_caption(&cap("Hello world"), &st, 1920, 1080).unwrap();
        // centred horizontally, in the lower part of the frame
        let cx = o.x as f32 + o.w as f32 / 2.0;
        assert!((cx - 960.0).abs() < 4.0, "centre {cx}");
        assert!(o.y > 800 && (o.y as usize + o.h) < 1080, "y {} h {}", o.y, o.h);
        // white text pixels exist and the box is dark translucent
        let mut white = 0;
        let mut boxy = 0;
        for p in o.px.chunks(4) {
            if p[3] > 0.99 && p[0] > 0.9 {
                white += 1;
            } else if p[3] > 0.7 && p[0] < 0.01 {
                boxy += 1;
            }
        }
        assert!(white > 500, "{white}");
        assert!(boxy > 1000, "{boxy}");
    }

    #[test]
    fn top_anchor_and_vtt_line() {
        let st = CaptionStyle { anchor: CaptionAnchor::Top, ..Default::default() };
        let o = render_caption(&cap("Top"), &st, 1280, 720).unwrap();
        assert!(o.y < 100);
        let mut c = cap("Line");
        c.settings = "line:50% align:start".into();
        let o = render_caption(&c, &CaptionStyle::default(), 1280, 720).unwrap();
        assert!((o.y - 360).abs() < 4, "{}", o.y);
        assert!(o.x < 120, "{}", o.x);
    }

    #[test]
    fn margin_is_a_fraction_of_frame_height() {
        // #1542: on a portrait sequence, a bottom-anchored caption with margin 0.31 sits with
        // its bottom edge 0.31 of the frame height above the bottom; the top anchor mirrors it.
        let st = CaptionStyle { anchor: CaptionAnchor::Bottom, margin: 0.31, size: 78.0, ..Default::default() };
        let o = render_caption(&cap("Hello world"), &st, 1080, 1920).unwrap();
        // the overlay pads the content by the outline grow (the default outline is 0, so by 1 px)
        let bottom = (o.y as usize + o.h) as f32;
        let expected = 1920.0 - 0.31 * 1920.0;
        assert!((bottom - expected).abs() <= 2.0, "bottom edge {bottom}, expected {expected}");
        let st = CaptionStyle { anchor: CaptionAnchor::Top, margin: 0.31, size: 78.0, ..Default::default() };
        let o = render_caption(&cap("Hello world"), &st, 1080, 1920).unwrap();
        assert!((o.y as f32 - 0.31 * 1920.0).abs() <= 2.0, "top edge {}, expected {}", o.y, 0.31 * 1920.0);
        // margin 0 and the 0.45 clamp stay inside the frame
        for margin in [0.0, 0.31, 0.45, 9.0] {
            let st = CaptionStyle { anchor: CaptionAnchor::Bottom, margin, size: 78.0, ..Default::default() };
            let o = render_caption(&cap("Hello world"), &st, 1080, 1920).unwrap();
            // the outline-grow padding plus ceil rounding may reach 2 px past the edge; the
            // composite clips it
            assert!(o.y >= 0 && o.y as usize + o.h <= 1922, "margin {margin}: y {} h {}", o.y, o.h);
        }
    }

    #[test]
    fn wraps_and_composites() {
        let long = "word ".repeat(60);
        let o = render_caption(&cap(&long), &CaptionStyle::default(), 640, 360).unwrap();
        assert!(o.w <= 640 && o.h > 40, "{}x{}", o.w, o.h);
        let mut canvas = vec![0.0f32; 640 * 360 * 4];
        o.composite_onto(&mut canvas, 640, 360);
        assert!(canvas.chunks(4).any(|p| p[0] > 0.9));
    }

    #[test]
    fn outline_draws_around_text() {
        let st = CaptionStyle { background: false, outline: 4.0, outline_color: [255, 0, 0, 255], ..Default::default() };
        let o = render_caption(&cap("O"), &st, 1920, 1080).unwrap();
        assert!(o.px.chunks(4).any(|p| p[0] > 0.9 && p[1] < 0.05 && p[3] > 0.9), "red outline pixels");
    }

    #[test]
    fn shadow_draws_behind_the_text() {
        // a hard shadow is a copy of the glyphs, offset: black pixels appear where the text is not
        let st = CaptionStyle {
            background: false,
            outline: 0.0,
            shadow: true,
            shadow_color: [0, 0, 0, 255],
            shadow_offset_x: 6.0,
            shadow_offset_y: 6.0,
            shadow_softness: 0.0,
            ..Default::default()
        };
        let o = render_caption(&cap("Shadow"), &st, 1920, 1080).unwrap();
        let at = |x: usize, y: usize| {
            let j = (y * o.w + x) * 4;
            [o.px[j], o.px[j + 1], o.px[j + 2], o.px[j + 3]]
        };
        // walk down-right from a text pixel until the shadow shows up past the glyphs
        let mut found = false;
        for y in 0..o.h {
            for x in 0..o.w.saturating_sub(8) {
                let p = at(x, y);
                // white glyph
                if p[0] > 0.9 && p[1] > 0.9 && p[3] > 0.9 {
                    let q = at(x + 8, y + 2);
                    // black shadow to its right (the glyph may be there too; the shadow is darker)
                    if q[0] < 0.1 && q[3] > 0.5 {
                        found = true;
                    }
                }
            }
        }
        assert!(found, "offset dark shadow pixels next to the glyphs");
        // the shadow grows the overlay beyond the text alone
        let plain = CaptionStyle { shadow: false, ..st.clone() };
        let b = render_caption(&cap("Shadow"), &plain, 1920, 1080).unwrap();
        assert!(o.w > b.w && o.h > b.h, "{}x{} vs {}x{}", o.w, o.h, b.w, b.h);
        // softness blurs the edge: fewer fully opaque shadow pixels than a hard copy
        let soft = CaptionStyle { shadow_softness: 12.0, ..st.clone() };
        let s = render_caption(&cap("Shadow"), &soft, 1920, 1080).unwrap();
        let hard_black = o.px.chunks(4).filter(|p| p[0] < 0.05 && p[3] > 0.9).count();
        let soft_black = s.px.chunks(4).filter(|p| p[0] < 0.05 && p[3] > 0.9).count();
        assert!(soft_black < hard_black, "hard {hard_black} soft {soft_black}");
    }

    #[test]
    fn track_font_is_used() {
        // the style's font used to be ignored (always Inter)
        let inter = render_caption(&cap("Hello world"), &CaptionStyle::default(), 1920, 1080).unwrap();
        let serif = CaptionStyle { font: "Noto Serif".into(), ..Default::default() };
        let serif = render_caption(&cap("Hello world"), &serif, 1920, 1080).unwrap();
        assert_ne!((inter.w, &inter.px), (serif.w, &serif.px), "Noto Serif renders like Inter");
        let mono = CaptionStyle { font: "jetbrains mono".into(), ..Default::default() };
        let mono = render_caption(&cap("Hello world"), &mono, 1920, 1080).unwrap();
        assert_ne!((inter.w, &inter.px), (mono.w, &mono.px), "JetBrains Mono renders like Inter");
    }

    #[test]
    fn the_track_font_style_reaches_the_face() {
        // the style name went into the layout as SemiBold before the track could say otherwise
        let st = CaptionStyle { style: "Regular".into(), ..Default::default() };
        assert_eq!(caption_style(&st.font, &st.style, 54.0).style, "Regular");
        let bold = CaptionStyle { style: "Bold".into(), ..Default::default() };
        assert_eq!(caption_style(&bold.font, &bold.style, 54.0).style, "Bold");
        // an empty name keeps the historical SemiBold
        assert_eq!(caption_style("", "", 54.0).style, "SemiBold");
        assert_eq!(caption_style("Inter", "  ", 54.0).style, "SemiBold");
    }

    #[test]
    fn the_style_offsets_move_the_block_inside_the_frame() {
        let plain = CaptionStyle { background: false, outline: 0.0, ..Default::default() };
        let base = render_caption(&cap("Hello"), &plain, 1280, 720).unwrap();
        let up = CaptionStyle { offset_y: -0.3, ..plain.clone() };
        let up = render_caption(&cap("Hello"), &up, 1280, 720).unwrap();
        assert!(up.y < base.y, "offset_y -0.3 moves the block up ({} -> {})", base.y, up.y);
        let right = CaptionStyle { offset_x: 0.2, ..plain.clone() };
        let right = render_caption(&cap("Hello"), &right, 1280, 720).unwrap();
        assert!(right.x > base.x, "offset_x 0.2 moves the block right ({} -> {})", base.x, right.x);
        // a nudge that would leave the frame is clamped, not drawn off-canvas
        let far = CaptionStyle { offset_x: 1.0, offset_y: 1.0, ..plain.clone() };
        let far = render_caption(&cap("Hello"), &far, 1280, 720).unwrap();
        assert!(far.x >= 0 && far.x + far.w as i32 <= 1280, "x inside the frame");
        assert!(far.y >= 0 && far.y + far.h as i32 <= 720, "y inside the frame");
    }

    #[test]
    fn unknown_or_empty_font_falls_back_to_inter() {
        let inter = render_caption(&cap("Hello"), &CaptionStyle::default(), 1280, 720).unwrap();
        for font in ["", "  ", "No Such Font 123"] {
            let st = CaptionStyle { font: font.into(), ..Default::default() };
            assert_eq!(render_caption(&cap("Hello"), &st, 1280, 720), Some(inter.clone()), "font {font:?}");
        }
    }

    #[test]
    fn hidden_track_draws_nothing() {
        let mut t = CaptionTrack::new(TrackId(1), "S".into(), CaptionFormat::Subtitle);
        t.captions.push(cap("x"));
        assert!(track_overlay(&t, Tick(5), 100, 100).is_some());
        t.enabled = false;
        assert!(track_overlay(&t, Tick(5), 100, 100).is_none());
    }
}
