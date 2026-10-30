use super::layout::Layout;
use super::{ClipId, FilmcraftApp, Grab, Hit, Pos2, SNAP_PX, SNAP_REARM_PX, SNAP_RELEASE_PX, Sequence, Tick, Value, grab_at, json};

/// Which end of the dragged block a snap point is for: the sticky release is kept per end, so the
/// two ends snap independently.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SnapSide {
    Start,
    End,
}

impl SnapSide {
    fn slot(self) -> usize {
        match self {
            SnapSide::Start => 0,
            SnapSide::End => 1,
        }
    }
}

/// Snap `t` to nearby candidates (edits, playhead, markers, in/out). Returns snapped tick.
pub(in super::super) fn snap(app: &mut FilmcraftApp, seq: &Sequence, layout: &Layout, t: Tick, exclude: &[ClipId], mods: egui::Modifiers) -> Tick {
    snap_to(app, seq, layout, t, exclude, true, None, mods)
}

/// [`snap`] for one end of the dragged block, with the sticky hysteresis that end keeps.
pub(in super::super) fn snap_side(
    app: &mut FilmcraftApp,
    seq: &Sequence,
    layout: &Layout,
    t: Tick,
    exclude: &[ClipId],
    side: SnapSide,
    mods: egui::Modifiers,
) -> Tick {
    snap_to(app, seq, layout, t, exclude, true, Some(side), mods)
}

/// Where a dragged playhead lands: [`snap`], but not onto itself, or it would stick where it is
/// and only move in jumps of `SNAP_PX` (#164).
pub(in super::super) fn snap_playhead(app: &mut FilmcraftApp, seq: &Sequence, layout: &Layout, t: Tick, mods: egui::Modifiers) -> Tick {
    snap_to(app, seq, layout, t, &[], false, None, mods)
}

fn snap_to(
    app: &mut FilmcraftApp,
    seq: &Sequence,
    layout: &Layout,
    t: Tick,
    exclude: &[ClipId],
    to_playhead: bool,
    side: Option<SnapSide>,
    mods: egui::Modifiers,
) -> Tick {
    // Snapping is off, or Option (Alt) is held: Alt is read every frame, so it can be pressed and
    // let go in the middle of a drag to place the clip freely.
    if !app.session.state.snapping || mods.alt {
        return t;
    }
    let mut cands: Vec<Tick> = Vec::with_capacity(64);
    for tr in seq.all_tracks() {
        for it in &tr.items {
            if exclude.contains(&it.id) {
                continue;
            }
            cands.push(it.start);
            cands.push(it.end());
        }
    }
    if to_playhead {
        cands.push(app.session.playhead());
    }
    cands.extend(seq.markers.iter().map(|m| m.start));
    cands.extend(seq.mark_in);
    cands.extend(seq.mark_out);
    let x = layout.x_of(t);
    if let Some(slot) = side.map(SnapSide::slot) {
        // Sticky: stay on the point while the pointer is still near it, however far the block has
        // moved, so a drag along an edge does not flicker in and out of the snap.
        if let Some(stuck) = app.tl.snap_stick[slot] {
            if (layout.x_of(stuck) - x).abs() <= SNAP_RELEASE_PX {
                app.tl.snap_x = Some(layout.x_of(stuck));
                return stuck;
            }
            app.tl.snap_stick[slot] = None;
            app.tl.snap_free[slot].push(stuck);
        }
        // A point the drag broke free of stays out of reach until the pointer is well past it.
        app.tl.snap_free[slot].retain(|c| (layout.x_of(*c) - x).abs() < SNAP_REARM_PX);
    }
    let mut best: Option<(f32, Tick)> = None;
    for c in cands {
        if side.is_some_and(|s| app.tl.snap_free[s.slot()].contains(&c)) {
            continue;
        }
        let d = (layout.x_of(c) - x).abs();
        if d < SNAP_PX && best.is_none_or(|b| d < b.0) {
            best = Some((d, c));
        }
    }
    match best {
        Some((_, c)) => {
            app.tl.snap_x = Some(layout.x_of(c));
            if let Some(slot) = side.map(SnapSide::slot) {
                app.tl.snap_stick[slot] = Some(c);
            }
            c
        }
        None => t,
    }
}

/// What the Timeline shows at `pos` for the control channel's `ui.timeline.hit`, with `kind`: the
/// trim a press of the current tool with `mods` starts there (null when it would not trim).
pub fn hit_json(app: &FilmcraftApp, pos: Pos2, mods: egui::Modifiers) -> Value {
    let (Some(layout), Some(seq)) = (app.tl.layout.as_ref(), app.session.active_sequence()) else { return json!({"hit": "none"}) };
    let t = layout.tick_at(pos.x);
    let grab = grab_at(seq, layout, pos, app.ui.tool, mods, app.session.prefs.trim.selection_tool_roll_ripple);
    let kind = match grab {
        Grab::Edge { kind, .. } => json!(kind.name()),
        Grab::Volume { .. } => json!("volume"),
        Grab::Other(_) => Value::Null,
    };
    match grab.hit() {
        Hit::Ruler => json!({"hit": "ruler", "time": t.0}),
        Hit::WorkArea { edge } => json!({"hit": "workArea", "edge": edge.map(|e| format!("{e:?}")), "time": t.0}),
        Hit::Clip { track, clip, edge } => {
            json!({"hit": "clip", "track": track.0, "clip": clip.0, "edge": edge.map(|e| format!("{e:?}")), "kind": kind, "time": t.0})
        }
        Hit::Transition { track, id, edge } => json!({"hit": "transition", "track": track.0, "transition": id.0, "edge": edge.map(|e| format!("{e:?}"))}),
        Hit::Empty { track } => json!({"hit": "empty", "track": track.0, "time": t.0}),
        Hit::None => json!({"hit": "none"}),
    }
}

/// Screen point of a clip (centre, or its in/out edge).
pub fn locate(app: &FilmcraftApp, clip: u64, edge: Option<&str>) -> Option<(f32, f32)> {
    let layout = app.tl.layout.as_ref()?;
    let seq = app.session.active_sequence()?;
    let (tid, it) = seq.find_item(ClipId(clip))?;
    let row = layout.rows.iter().find(|r| r.track == tid)?;
    let y = row.rect.center().y;
    let x = match edge {
        Some("in") => layout.x_of(it.start) + 2.0,
        Some("out") => layout.x_of(it.end()) - 2.0,
        _ => (layout.x_of(it.start) + layout.x_of(it.end())) / 2.0,
    };
    Some((x, y))
}
