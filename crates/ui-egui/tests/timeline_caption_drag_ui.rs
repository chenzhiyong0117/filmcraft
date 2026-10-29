//! Headless UI test of box selection across caption tracks and clips, and of dragging the mixed
//! selection: a marquee writes both selections, and dragging any selected caption moves the clips
//! with it in one edit (one undo step).

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_project::ClipId;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

const PPS: f64 = 40.0;

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
    frame_s: f64,
}

impl Driver {
    fn demo() -> Self {
        let mut session = Session::default();
        session.execute("file.openDemoProject", json!({})).expect("demo project");
        let (tx, rx) = channel();
        let app = FilmcraftApp::new(session).with_control(rx);
        let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
        let mut d = Driver { harness, tx, frame_s: 1.0 / 24.0 };
        d.frames(4);
        d
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            let ctx = self.harness.ctx.clone();
            let mut raw = std::mem::take(self.harness.input_mut());
            eframe::App::raw_input_hook(self.harness.state_mut(), &ctx, &mut raw);
            *self.harness.input_mut() = raw;
            self.harness.step();
        }
    }

    fn ok(&mut self, method: &str, params: Value) -> Value {
        let (req, reply) = ControlRequest::new(method, params.clone());
        self.tx.send(req).expect("control channel");
        for _ in 0..600 {
            self.frames(1);
            if let Ok(v) = reply.try_recv() {
                assert_eq!(v["ok"], json!(true), "{method} {params} failed: {v}");
                return v["result"].clone();
            }
        }
        panic!("no reply to {method} {params}");
    }

    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({"command": command, "params": params}))
    }

    fn rect(&mut self, id: &str) -> egui::Rect {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        let e = v.as_array().and_then(|a| a.iter().find(|e| e["id"] == id)).unwrap_or_else(|| panic!("no element {id}")).clone();
        let r: Vec<f64> = e["rect"].as_array().expect("rect").iter().map(|x| x.as_f64().expect("number")).collect();
        egui::Rect::from_min_size(egui::pos2(r[0] as f32, r[1] as f32), egui::vec2(r[2] as f32, r[3] as f32))
    }

    /// Pointer x travel for `frames` frames of timeline time at the test's fixed zoom.
    fn px(&self, frames: i64) -> f32 {
        (frames as f64 * self.frame_s * PPS) as f32
    }

    fn send(&mut self, e: egui::Event) {
        self.harness.input_mut().events.push(e);
        self.frames(1);
    }

    fn button(&mut self, pos: egui::Pos2, pressed: bool) {
        self.send(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE });
    }

    fn move_to(&mut self, from: egui::Pos2, to: egui::Pos2) {
        for k in 1..=8 {
            self.send(egui::Event::PointerMoved(from.lerp(to, k as f32 / 8.0)));
        }
    }

    fn start_of(&self, clip: ClipId) -> i64 {
        self.harness.state().session.active_sequence().and_then(|q| q.find_item(clip)).map(|(_, i)| i.start.0).unwrap_or(-1)
    }

    fn caption_start(&self, cap: ClipId) -> i64 {
        self.harness.state().session.active_sequence().and_then(|q| q.find_caption(cap)).map(|(_, c)| c.start.0).unwrap_or(-1)
    }

    fn tick_of(&self, frames: i64) -> i64 {
        self.harness.state().session.sequence_rate().tick_of(frames).0
    }

    /// One 48-frame clip at the sequence start, and a 12-frame caption on the first 12 frames.
    fn setup(&mut self) -> (ClipId, ClipId) {
        self.exec("file.newSequence", json!({"name": "Box", "video": 1, "audio": 0}));
        let item = {
            let s = &self.harness.state().session;
            s.project.items.values().find(|i| i.has_video() && i.as_media().is_some()).expect("a media item").id
        };
        self.frame_s = self.harness.state().session.sequence_rate().frame_duration().seconds();
        let duration = self.tick_of(48);
        self.exec("timeline.place", json!({"item": item.0, "frame": 0, "duration": duration}));
        let d12 = self.tick_of(12) as f64 / filmcraft_time::TICKS_PER_SECOND as f64;
        let cap = self.exec("captions.add", json!({"frame": 0, "durationSeconds": d12, "text": "Box me"}))["caption"].as_u64().unwrap();
        self.ok("ui.set", json!({"focused": "Timeline", "timeline": {"pps": PPS, "scroll": 0.0}}));
        self.frames(3);
        let clip = self.harness.state().session.active_sequence().unwrap().video_tracks[0].items[0].id;
        (clip, ClipId(cap))
    }
}

#[test]
fn dragging_a_selected_caption_moves_the_clips_with_it_in_one_undo_step() {
    let mut d = Driver::demo();
    let (clip, cap) = d.setup();
    let cap_rect = d.rect(&format!("timeline.caption.{}", cap.0));
    let clip_rect = d.rect(&format!("timeline.clip.{}", clip.0));
    // a box over the caption and the clip
    let start = egui::pos2(cap_rect.max.x + 6.0, cap_rect.center().y);
    let end = egui::pos2(cap_rect.center().x, clip_rect.center().y);
    d.send(egui::Event::PointerMoved(start));
    d.button(start, true);
    d.move_to(start, end);
    d.button(end, false);
    d.frames(2);
    assert_eq!(d.harness.state().session.state.caption_selection, vec![cap]);
    // drag the selected caption block 24 frames to the right
    let from = cap_rect.center();
    let dx = d.px(24);
    d.send(egui::Event::PointerMoved(from));
    d.button(from, true);
    d.move_to(from, from + egui::vec2(dx, 0.0));
    d.button(from + egui::vec2(dx, 0.0), false);
    d.frames(2);
    let shift = d.tick_of(24);
    assert_eq!(d.caption_start(cap), shift, "the caption moved by the drag");
    assert_eq!(d.start_of(clip), shift, "and the clip moved with it");
    // one undo step puts both back
    d.exec("edit.undo", json!({}));
    assert_eq!(d.caption_start(cap), 0);
    assert_eq!(d.start_of(clip), 0);
}

#[test]
fn dragging_a_selected_clip_moves_the_captions_with_it_too() {
    let mut d = Driver::demo();
    let (clip, cap) = d.setup();
    let cap_rect = d.rect(&format!("timeline.caption.{}", cap.0));
    let clip_rect = d.rect(&format!("timeline.clip.{}", clip.0));
    let start = egui::pos2(cap_rect.max.x + 6.0, cap_rect.center().y);
    let end = egui::pos2(cap_rect.center().x, clip_rect.center().y);
    d.send(egui::Event::PointerMoved(start));
    d.button(start, true);
    d.move_to(start, end);
    d.button(end, false);
    d.frames(2);
    // drag the selected picture 24 frames right
    let from = clip_rect.center();
    let dx = d.px(24);
    d.send(egui::Event::PointerMoved(from));
    d.button(from, true);
    d.move_to(from, from + egui::vec2(dx, 0.0));
    d.button(from + egui::vec2(dx, 0.0), false);
    d.frames(2);
    let shift = d.tick_of(24);
    assert_eq!(d.start_of(clip), shift, "the clip moved by the drag");
    assert_eq!(d.caption_start(cap), shift, "the caption came along in the same edit");
    d.exec("edit.undo", json!({}));
    assert_eq!(d.caption_start(cap), 0);
    assert_eq!(d.start_of(clip), 0);
}
