//! Clicking a caption block in the timeline scrolls the Text panel's caption list to that caption
//! (#caption-click): the block and the text field it edits stay in view together.

use std::sync::mpsc::{Sender, channel};

use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::FilmcraftApp;
use filmcraft_ui_egui::control::ControlRequest;
use serde_json::{Value, json};

struct Driver {
    harness: Harness<'static, FilmcraftApp>,
    tx: Sender<ControlRequest>,
}

impl Driver {
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
            if let Ok(r) = reply.try_recv() {
                assert_eq!(r["ok"], true, "{method} {params}: {r}");
                return r["result"].clone();
            }
        }
        panic!("no reply to {method} {params}");
    }
    fn exec(&mut self, command: &str, params: Value) -> Value {
        self.ok("engine.execute", json!({"command": command, "params": params}))
    }
    fn ids(&mut self, prefix: &str) -> Vec<String> {
        let v = self.ok("ui.elements", json!({"prefix": prefix}));
        v.as_array().expect("elements").iter().filter_map(|e| e["id"].as_str().map(str::to_string)).collect()
    }
    /// The rect of one automation element, by exact id.
    fn rect(&mut self, id: &str) -> [f32; 4] {
        let v = self.ok("ui.elements", json!({"prefix": id}));
        let list = v.as_array().expect("elements");
        let e = list.iter().find(|e| e["id"] == id).unwrap_or_else(|| panic!("{id} missing: {list:?}"));
        let r = e["rect"].as_array().expect("rect");
        let f = |i: usize| r[i].as_f64().expect("coord") as f32;
        [f(0), f(1), f(2), f(3)]
    }
}

#[test]
fn clicking_a_caption_block_scrolls_the_list_to_its_row() {
    let dir = std::env::temp_dir().join(format!("filmcraft-caption-reveal-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    // forty captions: the last one's row is far below the list's viewport
    let mut srt = String::new();
    for i in 0..40 {
        let start = 0.5 + i as f64;
        let end = start + 0.9;
        let stamp = |t: f64| {
            let ms = (t * 1000.0).round() as u64;
            format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, (ms / 60_000) % 60, (ms / 1000) % 60, ms % 1000)
        };
        srt.push_str(&format!("{}\n{} --> {}\nline {}\n\n", i + 1, stamp(start), stamp(end), i + 1));
    }
    let path = dir.join("forty.srt");
    std::fs::write(&path, srt).expect("write srt");

    let mut session = Session::default();
    session.execute("file.openDemoProject", json!({})).expect("demo project");
    let (tx, rx) = channel();
    let app = FilmcraftApp::new(session).with_control(rx);
    let harness = Harness::builder().with_size(egui::vec2(1600.0, 980.0)).with_max_steps(10_000).build_eframe(move |_cc| app);
    let mut d = Driver { harness, tx };
    d.frames(4);

    d.exec("captions.import", json!({"path": path.to_string_lossy()}));
    let caps = d.exec("captions.list", json!({}))["tracks"][0]["captions"].as_array().expect("captions").clone();
    assert_eq!(caps.len(), 40, "{caps:?}");
    let first = caps[0]["id"].as_u64().expect("id");
    let _ = first;

    d.ok("ui.panel.show", json!({"panel": "Text"}));
    d.frames(2);
    d.ok("ui.click", json!({"id": "text.tab.Captions"}));
    d.frames(2);

    // the timeline only draws the blocks in its current zoom, and a caption shorter than a pixel is
    // drawn too thin to click: take the last block that is wide enough, whose row is below the list
    let mut target = None;
    let blocks = d.ids("timeline.caption.");
    for id in blocks.iter().rev() {
        let r = d.rect(id);
        if r[2] >= 6.0 && r[1] > 0.0 && r[1] < 980.0 {
            let cid: u64 = id.rsplit('.').next().and_then(|s| s.parse().ok()).expect("caption id in the id");
            if d.rect(&format!("text.captions.{cid}.row"))[1] > 470.0 {
                target = Some((id.clone(), cid));
                break;
            }
        }
    }
    let (block, target_id) = target.expect("a clickable caption block whose row is off-screen");
    let row = format!("text.captions.{target_id}.row");
    let before = d.rect(&row);
    assert!(before[1] > 470.0, "row should start below the list's viewport: {before:?}");

    // clicking its block in the timeline seeks to it and scrolls the list to the same caption
    d.ok("ui.click", json!({"id": block}));
    d.frames(3);
    let after = d.rect(&row);
    assert!(after[1] >= 100.0 && after[1] < 470.0, "row should be scrolled into the viewport: {after:?}");
    assert!(d.harness.state().session.state.caption_selection.iter().map(|c| c.0).collect::<Vec<_>>() == vec![target_id], "the clicked caption is selected");
    assert!(d.harness.state().ui.caption_reveal.is_none(), "the reveal request is consumed once applied");
    assert!(d.rect(&format!("text.captions.{first}.row"))[1] < 0.0, "the scrolled-away top row is above the list");

    let _ = std::fs::remove_dir_all(&dir);
}
