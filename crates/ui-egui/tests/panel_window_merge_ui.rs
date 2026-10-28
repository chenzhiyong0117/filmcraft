//! Two panels pulled out onto another display sit in one native window each. Merging them (the
//! panel menu, or `window.mergePanelWindow` on the control channel) puts both in one window with a
//! tab strip, at the destination window's place; splitting a tab out (`window.splitPanelWindow`)
//! gives the panel a window of its own again.

use egui::{Event, Modifiers, PointerButton, Rect, pos2, vec2};
use egui_kittest::Harness;
use filmcraft_engine::Session;
use filmcraft_ui_egui::{
    FilmcraftApp,
    dock::{DockNode, Floating, Layout, PanelKind, PanelWindow, SplitSize},
    menus,
};
use serde_json::json;

fn layout() -> Layout {
    Layout {
        root: DockNode::Split {
            vertical: false,
            size: SplitSize::FixedA(400.0),
            a: Box::new(DockNode::Tabs { panels: vec![PanelKind::Project, PanelKind::Info], active: 0 }),
            b: Box::new(DockNode::Tabs { panels: vec![PanelKind::Source, PanelKind::Program], active: 1 }),
        },
        floating: vec![],
        panel_windows: vec![],
    }
}

#[test]
fn merging_two_panel_windows_keeps_one_window_and_splitting_restores_two() {
    let ctx = egui::Context::default();
    let mut app = FilmcraftApp::new(Session::default());
    app.ui.dock = layout().root;
    app.ui.floating.clear();
    app.ui.panel_windows.clear();
    // Program on the second display and Source next to it: each alone in its own native window.
    let places = [(PanelKind::Program, [1920.0, 96.0, 640.0, 360.0]), (PanelKind::Source, [2600.0, 96.0, 640.0, 360.0])];
    for (panel, rect) in places {
        menus::invoke(&mut app, &ctx, "window.floatPanel", json!({"panel": panel.id(), "rect": rect})).expect("float");
        menus::invoke(&mut app, &ctx, "window.panelWindow", json!({"panel": panel.id(), "window": true})).expect("window");
        assert!(app.in_panel_window(panel));
    }
    assert_eq!(app.ui.panel_windows.len(), 2);
    // the panel menu offers the other window as a destination
    let targets = app.merge_targets(PanelKind::Source);
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].0, PanelKind::Program);
    assert!(!targets[0].1.is_empty());
    assert_eq!(app.merge_targets(PanelKind::Program)[0].0, PanelKind::Source);
    // merging Source into Program's window leaves one window showing both panels as tabs, where
    // the destination window was
    let merged = menus::invoke(&mut app, &ctx, "window.mergePanelWindow", json!({"panel": "Source", "into": "Program"})).expect("merge");
    assert_eq!(merged, json!({"panel": "Source", "into": "Program"}));
    assert_eq!(app.ui.panel_windows, vec![PanelWindow { panel: PanelKind::Source, rect: [1920.0, 96.0, 640.0, 360.0] }]);
    assert_eq!(app.ui.floating.len(), 1, "the emptied floating group is gone");
    assert_eq!(app.ui.floating[0].panels, vec![PanelKind::Program, PanelKind::Source]);
    assert_eq!(app.ui.floating[0].active, 1, "the moved panel is the active tab");
    assert!(app.in_panel_window(PanelKind::Program), "both panels show in the surviving window");
    assert!(app.in_panel_window(PanelKind::Source));
    assert!(app.merge_targets(PanelKind::Source).is_empty(), "a panel already in that window is no destination");
    assert!(app.can_split_panel_window(PanelKind::Source));
    // pulling the tab out again gives it a window of its own next to the merged one
    let split = menus::invoke(&mut app, &ctx, "window.splitPanelWindow", json!({"panel": "Source", "rect": [2600.0, 96.0, 640.0, 360.0]})).expect("split");
    assert_eq!(split, json!({"panel": "Source", "window": true}));
    assert_eq!(app.ui.floating.len(), 2);
    assert_eq!(app.ui.panel_windows.len(), 2);
    assert!(app.ui.panel_windows.iter().any(|w| w.panel == PanelKind::Source && w.rect == [2600.0, 96.0, 640.0, 360.0]));
    assert!(app.ui.panel_windows.iter().any(|w| w.panel == PanelKind::Program), "the rest of the group kept its window");
    assert!(app.in_panel_window(PanelKind::Program) && app.in_panel_window(PanelKind::Source));
    assert!(!app.can_split_panel_window(PanelKind::Source));
    // a merged group goes back to the main window as one floating group
    menus::invoke(&mut app, &ctx, "window.mergePanelWindow", json!({"panel": "Source", "into": "Program"})).expect("merge again");
    assert_eq!(app.ui.panel_windows.len(), 1);
    menus::invoke(&mut app, &ctx, "window.panelWindow", json!({"panel": "Source", "window": false})).expect("main window");
    assert!(app.ui.panel_windows.is_empty());
    assert_eq!(app.ui.floating.len(), 1);
    assert_eq!(app.ui.floating[0].panels.len(), 2);
}

#[test]
fn a_merge_into_a_main_window_group_stays_on_the_desktop() {
    let ctx = egui::Context::default();
    let mut app = FilmcraftApp::new(Session::default());
    app.ui.dock = layout().root;
    app.ui.floating.clear();
    app.ui.panel_windows.clear();
    // Source floats in the main window; Program is on the second display.
    menus::invoke(&mut app, &ctx, "window.floatPanel", json!({"panel": "Source", "rect": [100.0, 100.0, 420.0, 320.0]})).expect("float");
    menus::invoke(&mut app, &ctx, "window.floatPanel", json!({"panel": "Program", "rect": [140.0, 140.0, 420.0, 320.0]})).expect("float");
    menus::invoke(&mut app, &ctx, "window.panelWindow", json!({"panel": "Program", "window": true})).expect("window");
    assert_eq!(app.ui.panel_windows.len(), 1);
    menus::invoke(&mut app, &ctx, "window.mergePanelWindow", json!({"panel": "Source", "into": "Program"})).expect("merge");
    // the desktop window is the one place both panels can be together, so it survives
    assert_eq!(app.ui.panel_windows.len(), 1);
    assert_eq!(app.ui.panel_windows[0].panel, PanelKind::Source);
    assert_eq!(app.ui.panel_windows[0].rect, [140.0, 140.0, 420.0, 320.0]);
    assert_eq!(app.ui.floating.len(), 1);
    // explicitly asking for the main window keeps the group floating there instead
    menus::invoke(&mut app, &ctx, "window.mergePanelWindow", json!({"panel": "Source", "into": "Program", "window": false})).expect("merge");
    assert!(app.ui.panel_windows.is_empty());
    assert_eq!(app.ui.floating.len(), 1);
    assert_eq!(app.ui.floating[0].panels.len(), 2);
}

fn step(h: &mut Harness<'_, FilmcraftApp>) {
    let ctx = h.ctx.clone();
    let mut input = std::mem::take(h.input_mut());
    eframe::App::raw_input_hook(h.state_mut(), &ctx, &mut input);
    *h.input_mut() = input;
    h.step();
}

fn rect(h: &Harness<'_, FilmcraftApp>, id: &str) -> Rect {
    let element = h.state().auto.find(id).unwrap_or_else(|| panic!("missing {id}"));
    Rect::from_min_size(pos2(element.rect[0], element.rect[1]), vec2(element.rect[2], element.rect[3]))
}

fn click_at(h: &mut Harness<'_, FilmcraftApp>, pos: egui::Pos2) {
    h.event(Event::PointerMoved(pos));
    step(h);
    for pressed in [true, false] {
        h.event(Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE });
        step(h);
    }
    for _ in 0..3 {
        step(h);
    }
}

fn click(h: &mut Harness<'_, FilmcraftApp>, id: &str) {
    click_at(h, rect(h, id).center());
}

#[test]
fn the_panel_menu_merges_two_floating_groups_and_splits_a_tab_back_out() {
    let app = FilmcraftApp::new(Session::default());
    let mut h = Harness::builder().with_size(vec2(1200.0, 760.0)).build_eframe(move |_| app);
    for _ in 0..4 {
        step(&mut h);
    }
    h.state_mut().ui.dock = DockNode::Tabs { panels: vec![PanelKind::Project], active: 0 };
    h.state_mut().ui.floating = vec![
        Floating { panels: vec![PanelKind::Program], active: 0, rect: [160.0, 120.0, 420.0, 300.0] },
        Floating { panels: vec![PanelKind::Source], active: 0, rect: [620.0, 120.0, 420.0, 300.0] },
    ];
    h.state_mut().ui.panel_windows.clear();
    for _ in 0..3 {
        step(&mut h);
    }
    // the Source panel's menu lists the Program group as a destination
    click(&mut h, "panel.menu.Source");
    assert!(h.state().auto.find("panel.menu.Source.merge").is_some(), "the panel menu offers a merge");
    click(&mut h, "panel.menu.Source.merge");
    assert!(h.state().auto.find("panel.menu.Source.merge.Program").is_some(), "the destination window is listed");
    let destination = rect(&h, "panel.menu.Source.merge.Program");
    click_at(&mut h, destination.center());
    assert_eq!(h.state().ui.floating.len(), 1, "the two groups became one tab strip");
    assert_eq!(h.state().ui.floating[0].panels, vec![PanelKind::Program, PanelKind::Source]);
    assert_eq!(h.state().ui.floating[0].active, 1);
    // the active tab's menu can send it back out to a window of its own
    click(&mut h, "panel.menu.Source");
    assert!(h.state().auto.find("panel.menu.Source.splitWindow").is_some(), "the active tab can leave the group");
    click(&mut h, "panel.menu.Source.splitWindow");
    assert_eq!(h.state().ui.floating.len(), 2, "the tab left into a group of its own");
    assert_eq!(h.state().ui.panel_windows.len(), 1, "shown in a native window of its own");
    assert!(h.state().ui.panel_windows.iter().all(|w| w.panel == PanelKind::Source));
    assert!(!h.state().can_split_panel_window(PanelKind::Source));
    assert!(!h.state().can_split_panel_window(PanelKind::Program));
}

#[test]
fn merge_targets_name_the_destination_windows() {
    let ctx = egui::Context::default();
    let mut app = FilmcraftApp::new(Session::default());
    app.ui.dock = layout().root;
    app.ui.floating.clear();
    app.ui.panel_windows.clear();
    for (panel, rect) in
        [(PanelKind::Source, [100.0, 100.0, 420.0, 320.0]), (PanelKind::Program, [140.0, 140.0, 420.0, 320.0]), (PanelKind::Info, [180.0, 180.0, 420.0, 320.0])]
    {
        menus::invoke(&mut app, &ctx, "window.floatPanel", json!({"panel": panel.id(), "rect": rect})).expect("float");
    }
    // three floating groups: each has the other two as destinations, named after their panels
    let targets = app.merge_targets(PanelKind::Source);
    assert_eq!(targets.len(), 2);
    assert!(!targets.iter().any(|(into, _)| *into == PanelKind::Source));
    assert!(targets.iter().all(|(_, label)| !label.is_empty()));
    // merging keeps the destination's label to show what the window will hold
    menus::invoke(&mut app, &ctx, "window.mergePanelWindow", json!({"panel": "Source", "into": "Program"})).expect("merge");
    assert_eq!(app.merge_targets(PanelKind::Info).len(), 1);
    assert_eq!(app.merge_targets(PanelKind::Source)[0].0, PanelKind::Info);
    assert_eq!(app.merge_targets(PanelKind::Program)[0].0, PanelKind::Info);
    // the merged group is one destination for a third panel, not two
    menus::invoke(&mut app, &ctx, "window.mergePanelWindow", json!({"panel": "Info", "into": "Program"})).expect("merge");
    assert_eq!(app.ui.floating.len(), 1);
    assert_eq!(app.ui.floating[0].panels.len(), 3);
}
