//! Dragging the Source monitor's marked range onto the Timeline, including from a Source panel
//! that lives in its own native window on another display (bug: the drop did nothing there).

use egui::ViewportId;
use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_engine::project::ItemId;
use filmcraft_time::{Tick, TimeRange};
use filmcraft_ui_egui::FilmcraftApp;
use serde_json::json;

fn step(h: &mut Harness<'static, FilmcraftApp>) {
    let ctx = h.ctx.clone();
    let mut raw = std::mem::take(h.input_mut());
    eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut raw);
    *h.input_mut() = raw;
    ctx.request_repaint(); // Direct engine edits in a harness do not wake the native event loop.
    h.step();
}

fn harness(item: u64) -> Harness<'static, FilmcraftApp> {
    let mut s = Session::default();
    s.execute("file.openDemoProject", json!({})).unwrap();
    s.execute("file.newSequence", json!({"name":"Window drag test","width":1280,"height":720,"fps":24})).unwrap();
    s.execute("source.open", json!({"item":item})).unwrap();
    // snapping off keeps the drop frame a pure function of the pointer
    s.state.snapping = false;
    let mut h = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).build_eframe(move |_cc| FilmcraftApp::new(s));
    for _ in 0..4 {
        step(&mut h);
    }
    h
}

fn pointer(h: &mut Harness<'static, FilmcraftApp>, pos: egui::Pos2, pressed: Option<bool>) {
    h.input_mut().events.push(egui::Event::PointerMoved(pos));
    if pressed == Some(true) {
        step(h); // Establish hover before pressing, as the live control-channel drag does.
    }
    if let Some(pressed) = pressed {
        h.input_mut().events.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
    }
    step(h);
}

fn control_rect(h: &Harness<'static, FilmcraftApp>, id: &str) -> egui::Rect {
    let [x, y, w, height] = h.state().auto.elements.iter().find(|e| e.id == id).or_else(|| h.state().auto.find(id)).unwrap().rect;
    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, height))
}

/// Press the Source drag button, move the pointer over the given timeline track, release.
fn drag_to(h: &mut Harness<'static, FilmcraftApp>, control: &str, track: &str) -> egui::Pos2 {
    let from = control_rect(h, control).center();
    let panel = control_rect(h, "panel.Timeline");
    let row = control_rect(h, track);
    let to = egui::pos2(panel.min.x + h.state().ui.timeline.header_w + 60.0, row.center().y);
    pointer(h, from, Some(true));
    pointer(h, from + egui::vec2(0.0, 9.0), None);
    pointer(h, to, None);
    pointer(h, to, Some(false));
    for _ in 0..2 {
        step(h);
    }
    to
}

#[test]
fn marked_range_lands_on_v1_at_the_pointer_frame_with_its_exact_span() {
    let mut h = harness(5);
    let in_point = Tick::from_seconds_f64(1.0);
    let out_point = Tick::from_seconds_f64(3.0);
    h.state_mut().session.execute("project.setMarks", json!({"item":5,"in":in_point.0,"out":out_point.0})).unwrap();
    step(&mut h);
    let marked = filmcraft_engine::clip_ops::source_view(&h.state().session, ItemId(5)).unwrap().selected_range();
    // the drop lands on the frame under the pointer in the layout the drag sees; placing the first
    // clip re-fits the empty sequence's zoom, so read the layout before the press
    let rate = h.state().session.active_sequence().unwrap().settings.frame_rate;
    let layout = h.state().tl.layout.clone().expect("timeline laid out");
    let to = drag_to(&mut h, "source.drag.both", "timeline.track.V1.target");
    let expected = rate.snap_nearest(layout.tick_at(to.x).max(Tick::ZERO));
    // the clip's end lands on the frame grid (timeline.place), so the span may round by a frame
    let expected_duration = rate.snap_nearest(marked.start + marked.duration) - rate.snap_nearest(marked.start);
    let q = h.state().session.active_sequence().unwrap();
    assert_eq!(q.video_tracks[0].items.len(), 1, "status: {}", h.state().ui.status);
    let c = &q.video_tracks[0].items[0];
    assert_eq!(c.start, expected, "the clip starts at the frame under the pointer");
    assert_eq!(c.source_in, marked.start, "the drop keeps the marked in point");
    assert_eq!(c.duration, expected_duration, "the drop keeps the marked span");
    assert_eq!(in_point, marked.start);
}

#[test]
fn audio_only_drag_lands_on_the_audio_track_and_nowhere_else() {
    let mut h = harness(5);
    let in_point = Tick::from_seconds_f64(1.5);
    let out_point = Tick::from_seconds_f64(2.5);
    h.state_mut().session.execute("project.setMarks", json!({"item":5,"in":in_point.0,"out":out_point.0})).unwrap();
    step(&mut h);
    let marked = filmcraft_engine::clip_ops::source_view(&h.state().session, ItemId(5)).unwrap().selected_range();
    let rate = h.state().session.active_sequence().unwrap().settings.frame_rate;
    let layout = h.state().tl.layout.clone().expect("timeline laid out");
    let to = drag_to(&mut h, "source.drag.audio", "timeline.track.A1.target");
    let expected = rate.snap_nearest(layout.tick_at(to.x).max(Tick::ZERO));
    let expected_duration = rate.snap_nearest(marked.start + marked.duration) - rate.snap_nearest(marked.start);
    let q = h.state().session.active_sequence().unwrap();
    assert_eq!(q.video_tracks.iter().map(|t| t.items.len()).sum::<usize>(), 0, "status: {}", h.state().ui.status);
    assert_eq!(q.audio_tracks.iter().map(|t| t.items.len()).sum::<usize>(), 1);
    let c = &q.audio_tracks[0].items[0];
    assert_eq!(c.start, expected);
    assert_eq!((c.source_in, c.duration), (marked.start, expected_duration));
    assert_eq!(in_point, marked.start);
}

#[test]
fn releasing_outside_the_tracks_places_nothing() {
    let mut h = harness(5);
    let in_point = Tick::from_seconds_f64(1.0);
    h.state_mut().session.execute("project.setMarks", json!({"item":5,"in":in_point.0,"out":Tick::from_seconds_f64(3.0).0})).unwrap();
    step(&mut h);
    let panel = control_rect(&h, "panel.Timeline");
    // the panel's top strip is outside the track content, so the drop has no target
    let to = egui::pos2(panel.min.x + 8.0, panel.min.y + 6.0);
    let from = control_rect(&h, "source.drag.both").center();
    pointer(&mut h, from, Some(true));
    pointer(&mut h, from + egui::vec2(0.0, 9.0), None);
    pointer(&mut h, to, None);
    pointer(&mut h, to, Some(false));
    for _ in 0..2 {
        step(&mut h);
    }
    let q = h.state().session.active_sequence().unwrap();
    assert_eq!(q.video_tracks.iter().map(|t| t.items.len()).sum::<usize>(), 0, "status: {}", h.state().ui.status);
    assert_eq!(q.audio_tracks.iter().map(|t| t.items.len()).sum::<usize>(), 0);
    assert_eq!(h.state().session.project.item(ItemId(5)).and_then(|i| i.as_media()).and_then(|m| m.mark_in), Some(in_point));
}

fn root_viewport(min: egui::Pos2) -> egui::ViewportInfo {
    egui::ViewportInfo { inner_rect: Some(egui::Rect::from_min_size(min, egui::vec2(1200.0, 800.0))), ..Default::default() }
}

fn panel_viewport(min: egui::Pos2) -> egui::ViewportInfo {
    egui::ViewportInfo { inner_rect: Some(egui::Rect::from_min_size(min, egui::vec2(640.0, 400.0))), parent: Some(ViewportId::ROOT), ..Default::default() }
}

/// A Source panel in its own native window must hand its drag pointer to the main window: the
/// Timeline there never sees that window's pointer events. Drives the two viewport passes of one
/// egui context directly, the way `show_viewport_immediate` does on a native platform.
#[test]
fn a_panel_window_publishes_its_drag_pointer_in_main_window_coordinates() {
    use filmcraft_ui_egui::panels;
    use filmcraft_ui_egui::panels::source_drag::SourceDrag;

    let ctx = egui::Context::default();
    let child = ViewportId::from_hash_of("panel-window");
    let child_min = egui::pos2(1920.0, 66.0);
    // panel-window pass: start the drag, then publish that window's pointer
    let mut raw = egui::RawInput { viewport_id: child, ..Default::default() };
    raw.viewports.insert(child, panel_viewport(child_min));
    raw.events = vec![egui::Event::PointerMoved(egui::pos2(100.0, 80.0))];
    let mut started = false;
    let _ = ctx.run_ui(raw, |ui| {
        panels::start_drag_source(ui, SourceDrag { item: ItemId(1), range: TimeRange::new(Tick::ZERO, Tick::from_seconds_f64(2.0)), video: true, audio: true });
        panels::publish_window_drag(ui.ctx());
        started = true;
    });
    assert!(started, "the panel window pass ran");
    // main-window pass: the same drag pointer, translated into this window
    let mut raw = egui::RawInput { viewport_id: ViewportId::ROOT, ..Default::default() };
    raw.viewports.insert(ViewportId::ROOT, root_viewport(egui::pos2(0.0, 0.0)));
    raw.viewports.insert(child, panel_viewport(child_min));
    let _ = ctx.run_ui(raw, |ui| {
        let ctx = ui.ctx();
        assert_eq!(panels::window_drag_pointer(ctx), Some(egui::pos2(2020.0, 146.0)));
        assert!(!panels::window_drag_released(ctx));
        panels::clear_window_drag(ctx);
        assert_eq!(panels::window_drag_pointer(ctx), None);
    });
}

#[test]
fn a_release_in_a_panel_window_is_visible_to_the_main_window() {
    use filmcraft_ui_egui::panels;

    let ctx = egui::Context::default();
    let child = ViewportId::from_hash_of("panel-window");
    let child_min = egui::pos2(1920.0, 66.0);
    let mut raw = egui::RawInput { viewport_id: child, ..Default::default() };
    raw.viewports.insert(child, panel_viewport(child_min));
    raw.events = vec![
        egui::Event::PointerMoved(egui::pos2(40.0, 40.0)),
        egui::Event::PointerButton { pos: egui::pos2(40.0, 40.0), button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
    ];
    let _ = ctx.run_ui(raw, |ui| {
        panels::start_drag_source(
            ui,
            filmcraft_ui_egui::panels::source_drag::SourceDrag {
                item: ItemId(1),
                range: TimeRange::new(Tick::ZERO, Tick::from_seconds_f64(1.0)),
                video: true,
                audio: true,
            },
        );
        panels::publish_window_drag(ui.ctx());
    });
    let mut raw = egui::RawInput { viewport_id: child, ..Default::default() };
    raw.viewports.insert(child, panel_viewport(child_min));
    raw.events = vec![egui::Event::PointerButton {
        pos: egui::pos2(40.0, 40.0),
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    }];
    let _ = ctx.run_ui(raw, |ui| panels::publish_window_drag(ui.ctx()));
    let mut raw = egui::RawInput { viewport_id: ViewportId::ROOT, ..Default::default() };
    raw.viewports.insert(ViewportId::ROOT, root_viewport(egui::pos2(0.0, 0.0)));
    raw.viewports.insert(child, panel_viewport(child_min));
    let _ = ctx.run_ui(raw, |ui| {
        let ctx = ui.ctx();
        assert!(panels::window_drag_released(ctx), "the release in the panel window reaches the main window");
        panels::clear_window_drag(ctx);
        assert!(!panels::window_drag_released(ctx));
    });
}
