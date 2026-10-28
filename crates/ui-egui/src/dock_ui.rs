//! FilmCraft's panel chrome and document tabs over the shared docking transactions.
//!
//! A floating group can also move out of the main window into its own native window
//! ([`FilmcraftApp::set_panel_window`]), which can sit on another monitor.

use craft_ui::docking::{Action, DropTarget, Placement, Zone, resolve_drop};
use egui::{Color32, Rect, Sense, Stroke, StrokeKind, pos2, vec2};
use serde_json::{Value, json};

use crate::dock::{self, DockAction, DockNode, Group, PanelKind, PanelWindow};
use crate::{FilmcraftApp, panels};

/// Single source for rendering and hit testing: saved offscreen positions remain
/// reachable, and every panel has enough room for its chrome.
fn floating_rect(group: &dock::Floating, screen: Rect) -> Rect {
    let rect = Rect::from_min_size(pos2(group.rect[0], group.rect[1]), vec2(group.rect[2].max(160.0), group.rect[3].max(100.0)));
    rect.translate(vec2(
        (screen.min.x - rect.min.x).max(0.0) + (screen.max.x - 40.0 - rect.min.x).min(0.0),
        (screen.min.y - rect.min.y).max(0.0) + (screen.max.y - 30.0 - rect.min.y).min(0.0),
    ))
}

fn drag_id() -> egui::Id {
    egui::Id::new("dock-panel-drag")
}

/// The viewport a panel's own native window draws in.
fn panel_window_id(panel: PanelKind) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("panel-window", panel.id()))
}

/// Where a panel's native window opens first: kept for the session, so the window system owns
/// its place after that instead of being dragged back every frame.
fn panel_window_spawn_id(panel: PanelKind) -> egui::Id {
    egui::Id::new(("panel-window-spawn", panel.id()))
}

pub(crate) fn route(app: &mut FilmcraftApp, ctx: &egui::Context, id: &str, params: &Value) -> Option<Result<Value, String>> {
    if id == "window.dockAction" {
        return Some((|| {
            let value = params.get("action").cloned().ok_or("missing docking action")?;
            let action = serde_json::from_value(value).map_err(|error| format!("invalid docking action: {error}"))?;
            app.apply_dock(action)?;
            Ok(Value::Null)
        })());
    }
    if id == "window.panelWindow" {
        return Some((|| {
            let panel = match params.get("panel") {
                None => app.ui.focused,
                Some(value) => value.as_str().and_then(PanelKind::from_name).ok_or("unknown panel")?,
            };
            let window = match params.get("window") {
                None => None,
                Some(value) => Some(value.as_bool().ok_or("window must be a boolean")?),
            };
            let open = app.set_panel_window(ctx, panel, window)?;
            Ok(json!({"panel": panel.id(), "window": open}))
        })());
    }
    if id == "window.mergePanelWindow" {
        return Some((|| {
            let panel = match params.get("panel") {
                None => app.ui.focused,
                Some(value) => value.as_str().and_then(PanelKind::from_name).ok_or("unknown panel")?,
            };
            let into = params.get("into").and_then(Value::as_str).and_then(PanelKind::from_name).ok_or("into must name a floating panel")?;
            let window = match params.get("window") {
                None => None,
                Some(value) => Some(value.as_bool().ok_or("window must be a boolean")?),
            };
            app.merge_panel_window(ctx, panel, into, window)?;
            Ok(json!({"panel": panel.id(), "into": into.id()}))
        })());
    }
    if id == "window.splitPanelWindow" {
        return Some((|| {
            let panel = match params.get("panel") {
                None => app.ui.focused,
                Some(value) => value.as_str().and_then(PanelKind::from_name).ok_or("unknown panel")?,
            };
            let rect = match params.get("rect") {
                None => None,
                Some(value) => Some(serde_json::from_value::<[f32; 4]>(value.clone()).map_err(|_| "rect must contain four finite numbers")?),
            };
            let window = app.split_panel_window(ctx, panel, rect)?;
            Ok(json!({"panel": panel.id(), "window": window}))
        })());
    }
    if !matches!(id, "window.dockPanel" | "window.floatPanel" | "window.closePanel") {
        return None;
    }
    Some((|| {
        let panel = match params.get("panel") {
            None => app.ui.focused,
            Some(value) => value.as_str().and_then(PanelKind::from_name).ok_or("unknown panel")?,
        };
        let action = match id {
            "window.closePanel" => Action::Close { panel },
            "window.floatPanel" => {
                let rect = match params.get("rect") {
                    None => default_float_rect(ctx.content_rect()),
                    Some(value) => serde_json::from_value::<[f32; 4]>(value.clone()).map_err(|_| "rect must contain four finite numbers")?,
                };
                Action::Float { panel, rect }
            }
            _ => {
                let zone_name = match params.get("zone") {
                    None => "center",
                    Some(value) => value.as_str().ok_or("zone must be a string")?,
                };
                let zone = match zone_name.to_ascii_lowercase().as_str() {
                    "left" => Zone::Left,
                    "right" => Zone::Right,
                    "top" => Zone::Top,
                    "bottom" => Zone::Bottom,
                    "center" | "tab" => Zone::Center,
                    _ => return Err("unknown docking zone".into()),
                };
                let Some(anchor) = params.get("anchor") else {
                    if params.get("before").is_some() {
                        return Err("before needs an anchor panel".into());
                    }
                    app.dock_to_edge(panel, zone)?;
                    return Ok(json!({"panel": panel.id()}));
                };
                let anchor = anchor.as_str().and_then(PanelKind::from_name).ok_or("a known anchor panel is required")?;
                let placement = match params.get("before") {
                    Some(value) => Placement::Tab { before: Some(value.as_str().and_then(PanelKind::from_name).ok_or("unknown before panel")?) },
                    None => Placement::Split(zone),
                };
                Action::Move { panel, anchor, placement }
            }
        };
        app.apply_dock(action)?;
        Ok(json!({"panel": panel.id()}))
    })())
}

pub(crate) fn default_float_rect(screen: Rect) -> [f32; 4] {
    let center = screen.center();
    [center.x - 210.0, center.y - 160.0, 420.0, 320.0]
}

impl FilmcraftApp {
    fn dock_command(&mut self, ctx: &egui::Context, action: Action<PanelKind>) -> Result<(), String> {
        let value = serde_json::to_value(action).map_err(|error| error.to_string())?;
        crate::menus::invoke(self, ctx, "window.dockAction", json!({"action": value})).map(|_| ())
    }

    pub fn apply_dock(&mut self, action: Action<PanelKind>) -> Result<(), String> {
        let relocating = matches!(&action, Action::Move { .. } | Action::Float { .. });
        let reveal = match &action {
            Action::Activate { panel } => Some(*panel),
            _ => None,
        };
        // a panel closed while floating remembers where; closed while docked, it forgets (#1985)
        let closing = match &action {
            Action::Close { panel } => Some((*panel, self.ui.floating.iter().find(|g| g.panels.contains(panel)).map(|g| g.rect))),
            _ => None,
        };
        self.commit_dock(|shared| {
            crate::dock_shared::check_last_docked(shared, &action)?;
            shared.apply(action).map_err(|error| error.to_string())
        })?;
        if let Some(panel) = reveal {
            self.pending_panel_raise = Some(panel);
        }
        if let Some((panel, rect)) = closing {
            self.ui.closed_floating.retain(|(p, _)| *p != panel);
            if let Some(rect) = rect {
                self.ui.closed_floating.push((panel, rect));
            }
        }
        if relocating
            || self.ui.keys.maximized.is_some_and(|panel| !self.ui.dock.contains(panel) && !self.ui.floating.iter().any(|group| group.panels.contains(&panel)))
        {
            self.ui.keys.maximized = None;
        }
        Ok(())
    }

    /// Dock a panel along an edge of the whole workspace rather than beside one group.
    pub fn dock_to_edge(&mut self, panel: PanelKind, zone: Zone) -> Result<(), String> {
        self.commit_dock(|shared| crate::dock_shared::dock_at_edge(shared, panel, zone))?;
        self.ui.keys.maximized = None;
        Ok(())
    }

    /// The native window a floating group (its `panels`) shows in, if any.
    fn panel_window_of(&self, panels: &[PanelKind]) -> Option<PanelWindow> {
        self.ui.panel_windows.iter().find(|w| panels.contains(&w.panel)).copied()
    }

    /// Whether panel `p` shows in its own native window.
    pub fn in_panel_window(&self, p: PanelKind) -> bool {
        self.ui.floating.iter().any(|group| group.panels.contains(&p) && self.panel_window_of(&group.panels).is_some())
    }

    /// Move panel `p` (and the floating group it is in) into its own native window, which the
    /// user can place on another monitor, or back into the main window (`window` false; it floats
    /// there where it was). `None` toggles. A docked panel is undocked first. The window opens
    /// over the place the panel had in the main window. Returns whether `p` is now in a window.
    pub fn set_panel_window(&mut self, ctx: &egui::Context, p: PanelKind, window: Option<bool>) -> Result<bool, String> {
        let group_of = |app: &Self| app.ui.floating.iter().find(|group| group.panels.contains(&p)).map(|group| (group.panels.clone(), group.rect));
        let current = self.in_panel_window(p);
        let window = window.unwrap_or(!current);
        if window == current {
            return Ok(current);
        }
        if !window {
            if let Some((panels, _)) = group_of(self) {
                self.ui.panel_windows.retain(|w| !panels.contains(&w.panel));
            }
            return Ok(false);
        }
        if group_of(self).is_none() {
            if !self.ui.dock.contains(p) {
                return Err(format!("{} is not open", p.title()));
            }
            let rect = default_float_rect(ctx.input_for(egui::ViewportId::ROOT, |i| i.content_rect()));
            self.apply_dock(Action::Float { panel: p, rect })?;
        }
        let Some((_, [x, y, w, h])) = group_of(self) else { return Err(format!("can't undock {}", p.title())) };
        // Over the panel's place in the main window (its content's position on the desktop).
        let origin = ctx.input_for(egui::ViewportId::ROOT, |i| i.viewport().inner_rect.map(|r| r.min)).unwrap_or(pos2(0.0, 0.0));
        let rect = PanelWindow::sanitized([origin.x + x, origin.y + y, w, h]);
        ctx.data_mut(|d| d.remove::<[f32; 4]>(panel_window_spawn_id(p)));
        self.ui.panel_windows.push(PanelWindow { panel: p, rect });
        if self.ui.keys.maximized == Some(p) {
            self.ui.keys.maximized = None;
        }
        Ok(true)
    }

    /// The floating group holding pane `p`, if any.
    fn floating_group_index(&self, p: PanelKind) -> Option<usize> {
        self.ui.floating.iter().position(|group| group.panels.contains(&p))
    }

    /// The other floating groups `p` can move into as a tab: the panel naming each destination
    /// window and the window's label, which is the name of the panels it shows.
    pub fn merge_targets(&self, p: PanelKind) -> Vec<(PanelKind, String)> {
        let own = self.floating_group_index(p);
        self.ui
            .floating
            .iter()
            .enumerate()
            .filter(|(index, _)| Some(*index) != own)
            .filter_map(|(_, group)| {
                let into = group.panels.get(group.active).or_else(|| group.panels.first())?;
                let names = group.panels.iter().map(|panel| crate::i18n::t(panel.title())).collect::<Vec<_>>().join(" · ");
                Some((*into, names))
            })
            .collect()
    }

    /// Whether `p` shares its floating group with other tabs, so it can leave for a window of its
    /// own ([`Self::split_panel_window`]).
    pub fn can_split_panel_window(&self, p: PanelKind) -> bool {
        self.floating_group_index(p).is_some_and(|index| self.ui.floating.get(index).is_some_and(|group| group.panels.len() > 1))
    }

    /// Move panel `p` into the floating group that holds `into` as a tab, so the two panels share
    /// one native window — the way two windows dragged onto a second display become one window
    /// with a tab strip. The merged window keeps the destination's place and size; `p`'s window
    /// is gone. With `window` false the merged group floats in the main window instead, with true
    /// it stays on the desktop; `None` keeps a desktop window when either panel had one. A docked
    /// `p` leaves the main window first.
    pub fn merge_panel_window(&mut self, ctx: &egui::Context, p: PanelKind, into: PanelKind, window: Option<bool>) -> Result<(), String> {
        if p == into {
            return Err(format!("{} is already there", p.title()));
        }
        if self.floating_group_index(p).is_none() {
            if !self.ui.dock.contains(p) {
                return Err(format!("{} is not open", p.title()));
            }
            let rect = default_float_rect(ctx.input_for(egui::ViewportId::ROOT, |i| i.content_rect()));
            self.apply_dock(Action::Float { panel: p, rect })?;
        }
        let Some(target_index) = self.floating_group_index(into) else {
            return Err(format!("{} is not a floating panel", into.title()));
        };
        let target = self.ui.floating[target_index].clone();
        let Some(anchor) = target.panels.get(target.active).or_else(|| target.panels.first()).copied() else {
            return Err(format!("{} has no floating group", into.title()));
        };
        if self.floating_group_index(p) == Some(target_index) {
            // already one tab strip: only the window can change
            if window == Some(false) {
                let panels = target.panels.clone();
                self.ui.panel_windows.retain(|w| !panels.contains(&w.panel));
            }
            return Ok(());
        }
        let origin = ctx.input_for(egui::ViewportId::ROOT, |i| i.viewport().inner_rect.map(|r| r.min)).unwrap_or(pos2(0.0, 0.0));
        let place = match window {
            Some(false) => None,
            _ => {
                let target_window = self.panel_window_of(&target.panels);
                let source_window =
                    self.floating_group_index(p).and_then(|index| self.ui.floating.get(index)).and_then(|group| self.panel_window_of(&group.panels));
                match (target_window.or(source_window), window) {
                    (Some(shown), _) => Some(shown.rect),
                    // a window was asked for where neither group had one: put it over the destination
                    (None, Some(true)) => Some(PanelWindow::sanitized([origin.x + target.rect[0], origin.y + target.rect[1], target.rect[2], target.rect[3]])),
                    (None, Some(_) | None) => None,
                }
            }
        };
        self.apply_dock(Action::Move { panel: p, anchor, placement: Placement::Tab { before: None } })?;
        let Some(merged) = self.ui.floating.iter().find(|group| group.panels.contains(&p) && group.panels.contains(&into)).cloned() else {
            return Err("the panels did not end up in one floating group".into());
        };
        // one window carries one floating group: the destination keeps its window, renamed to the
        // tab that is now active, and the window `p` came from is gone
        let mut windows: Vec<PanelWindow> = self.ui.panel_windows.iter().copied().filter(|w| !merged.panels.contains(&w.panel)).collect();
        if let Some(rect) = place {
            windows.push(PanelWindow { panel: p, rect: PanelWindow::sanitized(rect) });
        }
        crate::dock::Layout { root: self.ui.dock.clone(), floating: self.ui.floating.clone(), panel_windows: windows.clone() }.validate()?;
        self.ui.panel_windows = windows;
        ctx.data_mut(|data| data.remove::<[f32; 4]>(panel_window_spawn_id(p)));
        if let Some(rect) = place {
            let rect = PanelWindow::sanitized(rect);
            // the new window opens where the destination window was, not on the main display.
            // `p`'s own window keeps its viewport id, so the window system still has it: move and
            // resize it to the destination's place rather than opening a second window beside it.
            ctx.data_mut(|data| data.insert_temp(panel_window_spawn_id(p), rect));
            let id = panel_window_id(p);
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::OuterPosition(pos2(rect[0], rect[1])));
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::InnerSize(vec2(rect[2], rect[3])));
        }
        self.ui.focused = p;
        Ok(())
    }

    /// Pull panel `p` out of the floating group it shares with other tabs into a native window of
    /// its own; the rest of the group stays where it was. `rect` is the new window's place on the
    /// desktop; without it the window opens beside the group's window, or over the group's place
    /// in the main window. Returns whether `p` is in a window of its own.
    pub fn split_panel_window(&mut self, ctx: &egui::Context, p: PanelKind, rect: Option<[f32; 4]>) -> Result<bool, String> {
        let Some(index) = self.floating_group_index(p) else {
            // a docked panel leaves the main window directly
            return self.set_panel_window(ctx, p, Some(true));
        };
        let group = self.ui.floating[index].clone();
        if group.panels.len() <= 1 {
            return self.set_panel_window(ctx, p, Some(true));
        }
        let origin = ctx.input_for(egui::ViewportId::ROOT, |i| i.viewport().inner_rect.map(|r| r.min)).unwrap_or(pos2(0.0, 0.0));
        let global = match rect {
            Some(rect) => PanelWindow::sanitized(rect),
            None => match self.panel_window_of(&group.panels) {
                // beside the group's window, visible rather than hidden under it
                Some(window) => PanelWindow::sanitized([window.rect[0] + 28.0, window.rect[1] + 28.0, window.rect[2], window.rect[3]]),
                // over the panel's place in the main window
                None => PanelWindow::sanitized([origin.x + group.rect[0], origin.y + group.rect[1], group.rect[2], group.rect[3]]),
            },
        };
        let rest = group.panels.iter().copied().find(|panel| *panel != p);
        let windowed = self.panel_window_of(&group.panels);
        self.apply_dock(Action::Float { panel: p, rect: group.rect })?;
        // the part of the group that stays keeps its window; the window was named after the tab
        // that left, so it follows a panel that is still in the group
        if let (Some(anchor), Some(window)) = (rest, windowed)
            && window.panel == p
        {
            if let Some(w) = self.ui.panel_windows.iter_mut().find(|w| w.panel == p) {
                w.panel = anchor;
            }
            ctx.data_mut(|data| data.remove::<[f32; 4]>(panel_window_spawn_id(p)));
            ctx.data_mut(|data| data.insert_temp(panel_window_spawn_id(anchor), window.rect));
        }
        self.detach_panel(ctx, p, global)?;
        // `p`'s own viewport id named the merged window and survives the split: send it to its own
        // place instead of leaving it under the window the rest of the group keeps
        let id = panel_window_id(p);
        ctx.send_viewport_cmd_to(id, egui::ViewportCommand::OuterPosition(pos2(global[0], global[1])));
        ctx.send_viewport_cmd_to(id, egui::ViewportCommand::InnerSize(vec2(global[2], global[3])));
        self.ui.focused = p;
        Ok(true)
    }

    /// A drag that ends off the main window leaves the panel on the desktop as its own native
    /// window, at `global` (desktop points), sized by the caller: dropping a floating panel on
    /// the other display must not snap it back into the window. The panel floats first when it
    /// is still docked.
    pub(crate) fn detach_panel(&mut self, ctx: &egui::Context, panel: PanelKind, global: [f32; 4]) -> Result<(), String> {
        if !self.ui.floating.iter().any(|group| group.panels.contains(&panel)) {
            if !self.ui.dock.contains(panel) {
                return Err(format!("{} is not open", panel.title()));
            }
            let origin = ctx.input_for(egui::ViewportId::ROOT, |i| i.viewport().inner_rect.map(|r| r.min)).unwrap_or(pos2(0.0, 0.0));
            let rect = [global[0] - origin.x, global[1] - origin.y, global[2], global[3]];
            if !rect.iter().all(|v| v.is_finite()) {
                return Err("the drop position is not finite".into());
            }
            self.apply_dock(Action::Float { panel, rect })?;
        }
        let rect = PanelWindow::sanitized(global);
        ctx.data_mut(|data| data.remove::<[f32; 4]>(panel_window_spawn_id(panel)));
        self.ui.panel_windows.retain(|window| window.panel != panel);
        self.ui.panel_windows.push(PanelWindow { panel, rect });
        if self.ui.keys.maximized == Some(panel) {
            self.ui.keys.maximized = None;
        }
        Ok(())
    }

    fn commit_dock(&mut self, change: impl FnOnce(&mut craft_ui::docking::Layout<PanelKind>) -> Result<(), String>) -> Result<(), String> {
        let mut shared = crate::dock_shared::encode(&self.ui.dock, &self.ui.floating)?;
        shared.validate().map_err(|error| error.to_string())?;
        change(&mut shared)?;
        let layout = crate::dock_shared::decode(shared)?;
        crate::dock_shared::encode(&layout.root, &layout.floating)?;
        self.ui.dock = layout.root;
        self.ui.floating = layout.floating;
        Ok(())
    }

    fn panel_body(&mut self, ui: &mut egui::Ui, panel: PanelKind, content: Rect) {
        self.auto.add(&format!("panel.{}", panel.id()), content, panel.title());
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).id_salt(("panel", panel.id())));
        child.set_clip_rect(content.intersect(ui.clip_rect()));
        panels::show(self, &mut child, panel, content);
    }

    /// OS drag positions come from the host cursor hook, so egui pointer capture alone
    /// cannot decide whether a Timeline is covered. Use the same geometry and layer
    /// ordering as our renderer before any panel can dispatch an edit.
    pub(crate) fn timeline_drop_target(&self, ctx: &egui::Context, pos: egui::Pos2) -> bool {
        if !ctx.content_rect().contains(pos) {
            return false;
        }
        if let Some(panel) =
            self.ui.keys.maximized.filter(|panel| self.ui.dock.contains(*panel) || self.ui.floating.iter().any(|group| group.panels.contains(panel)))
        {
            return panel == PanelKind::Timeline;
        }
        for index in self.floating_order(ctx).into_iter().rev() {
            if let Some(group) = self.ui.floating.get(index)
                && floating_rect(group, ctx.content_rect()).contains(pos)
            {
                return group.panels.get(group.active) == Some(&PanelKind::Timeline);
            }
        }
        true
    }

    fn floating_order(&self, ctx: &egui::Context) -> Vec<usize> {
        let layers = ctx.memory(|memory| memory.layer_ids().collect::<Vec<_>>());
        let mut order: Vec<_> = (0..self.ui.floating.len()).collect();
        order.sort_by_key(|index| {
            self.ui
                .floating
                .get(*index)
                .and_then(|group| group.panels.first())
                .and_then(|panel| {
                    let id = egui::Id::new(("floating-panel", panel.id()));
                    layers.iter().position(|layer| layer.id == id)
                })
                .unwrap_or(usize::MAX)
        });
        order
    }

    pub(crate) fn dock_area(&mut self, ui: &mut egui::Ui, body: Rect) {
        let t = self.tokens;
        let ctx = ui.ctx().clone();
        if let Some(panel) = self.pending_panel_raise.take()
            && let Some(owner) = self.ui.floating.iter().find(|group| group.panels.contains(&panel)).and_then(|group| group.panels.first())
        {
            ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, egui::Id::new(("floating-panel", owner.id()))));
        }
        let maximized =
            self.ui.keys.maximized.filter(|panel| self.ui.dock.contains(*panel) || self.ui.floating.iter().any(|group| group.panels.contains(panel)));
        let mut dock = match maximized {
            Some(panel) => DockNode::Tabs { panels: vec![panel], active: 0 },
            None => std::mem::replace(&mut self.ui.dock, DockNode::Tabs { panels: vec![], active: 0 }),
        };
        let mut groups = vec![];
        let mut layout_actions = Vec::new();
        dock::layout_with_actions(ui, &mut dock, body, &t, "", &mut groups, &mut self.auto, &mut layout_actions);
        let seqs = dock::SeqTabs {
            open: self
                .session
                .state
                .open_sequences
                .iter()
                .filter_map(|id| {
                    self.session
                        .project
                        .item(*id)
                        .filter(|item| matches!(item.kind, filmcraft_project::ItemKind::Sequence(_)))
                        .map(|item| (id.0, item.name.clone()))
                })
                .collect(),
            active: self.session.state.active_sequence.map(|id| id.0),
        };
        let mut actions = vec![];
        let mut targets = vec![];
        for group in &groups {
            let chrome = dock::draw_group_chrome_details(ui, group, &seqs, &t, &mut self.auto);
            if let Some(target) = target(group, chrome.tabs, true) {
                targets.push(target);
            }
            actions.extend(chrome.actions);
        }
        if maximized.is_none() {
            self.ui.dock = dock;
        }
        for group in &groups {
            if let Some(panel) = group.panels.get(group.active).copied() {
                self.panel_body(ui, panel, group.content);
            }
        }
        for group in &groups {
            dock::paint_group_focus(ui, group, self.ui.focused, &t);
        }
        // A native window is kept only for a panel that still floats.
        let ui_state = &mut self.ui;
        ui_state.panel_windows.retain(|w| ui_state.floating.iter().any(|group| group.panels.contains(&w.panel)));
        let floating_order = self.floating_order(&ctx);
        for index in floating_order.into_iter().filter(|_| maximized.is_none()) {
            // Groups in their own native window take no tab drops from the main window.
            let window = self.ui.floating.get(index).and_then(|group| self.panel_window_of(&group.panels));
            if let Some(window) = window {
                actions.extend(self.panel_window(&ctx, index, window, &seqs));
                continue;
            }
            if let Some(target) = self.floating_group(&ctx, index, &seqs, &mut actions) {
                targets.push(target);
            }
        }
        for action in layout_actions {
            let _ = self.dock_command(&ctx, action);
        }
        for action in actions {
            match action {
                DockAction::Activate(panel) => {
                    let _ = self.dock_command(&ctx, Action::Activate { panel });
                }
                DockAction::Close(panel) => {
                    let _ = self.dock_command(&ctx, Action::Close { panel });
                }
                DockAction::Focus(panel) => self.ui.focused = panel,
                DockAction::BeginDrag(panel) => {
                    ctx.data_mut(|data| data.insert_temp(drag_id(), panel));
                }
                DockAction::OpenSequence(id) => {
                    let _ = self.dock_command(&ctx, Action::Activate { panel: PanelKind::Timeline });
                    if self.session.state.active_sequence.map(|item| item.0) != Some(id)
                        && let Err(error) = self.session.execute("sequence.open", json!({"item": id}))
                    {
                        self.ui.status = error.to_string();
                    }
                }
                DockAction::CloseSequence(id) => {
                    if let Err(error) = self.session.execute("sequence.close", json!({"item": id})) {
                        self.ui.status = error.to_string();
                    }
                }
                DockAction::MoveSequence(id, index) => {
                    if let Err(error) = self.session.execute("sequence.moveTab", json!({"item": id, "index": index})) {
                        self.ui.status = error.to_string();
                    }
                }
                DockAction::PanelMenu(panel, pos) => {
                    let frame = ctx.cumulative_frame_nr();
                    ctx.data_mut(|data| {
                        data.insert_temp(egui::Id::new("panel-menu"), (panel, pos));
                        data.insert_temp(egui::Id::new("panel-menu-opened"), frame);
                    });
                }
            }
        }
        self.panel_drag(&ctx, body, &targets);
        panels::panel_menu_popup(self, ui);
        self.cancel_value_drag(ui.ctx());
    }

    /// One floating group in its own native window: the group's tab strip and active panel fill
    /// the window. Closing the window brings the group back into the main window.
    fn panel_window(&mut self, ctx: &egui::Context, i: usize, window: PanelWindow, seqs: &dock::SeqTabs) -> Vec<DockAction> {
        let t = self.tokens;
        let Some(f) = self.ui.floating.get(i).cloned() else { return vec![] };
        let active = f.active.min(f.panels.len().saturating_sub(1));
        let Some(shown) = f.panels.get(active).copied() else { return vec![] };
        // The window opens at its saved place; after that the window system owns its position and
        // size (handing them back every frame would fight the user dragging it).
        let spawn_id = panel_window_spawn_id(window.panel);
        let spawn = ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(spawn_id, || PanelWindow::sanitized(window.rect)));
        let builder = egui::ViewportBuilder::default()
            .with_title(shown.title())
            .with_position([spawn[0], spawn[1]])
            .with_inner_size([spawn[2], spawn[3]])
            .with_min_inner_size(dock::PANEL_WINDOW_MIN);
        let mut closed = false;
        let mut placed = None;
        let mut actions = Vec::new();
        ctx.show_viewport_immediate(panel_window_id(window.panel), builder, |ctx, class| {
            let native = matches!(class, egui::ViewportClass::Immediate | egui::ViewportClass::Deferred);
            egui::CentralPanel::default().frame(egui::Frame::NONE.fill(t.panel_bg)).show(ctx, |ui| {
                let r = ui.max_rect();
                let group = Group {
                    path: format!("w{i}"),
                    rect: r,
                    content: Rect::from_min_max(pos2(r.min.x, (r.min.y + t.tab_h).min(r.max.y)), r.max),
                    panels: f.panels.clone(),
                    active,
                };
                let chrome = dock::draw_group_chrome_details(ui, &group, seqs, &t, &mut self.auto);
                // Tabs can't be dragged between native windows yet.
                actions.extend(chrome.actions.into_iter().filter(|a| !matches!(a, DockAction::BeginDrag(_))));
                if let Some(panel) = group.panels.get(group.active).copied() {
                    self.panel_body(ui, panel, group.content);
                }
                dock::paint_group_focus(ui, &group, self.ui.focused, &t);
                if native {
                    panels::panel_menu_popup(self, ui);
                }
            });
            // An embedded window (no native windows on this platform) reads the main window's
            // input: its close request and place aren't this window's.
            // Only this window's own viewport answers for it: on some frames the immediate
            // viewport reports the main window's input instead, and writing that rect back would
            // stretch the panel window to the size of the main window.
            let own = ctx.viewport_id() == panel_window_id(window.panel);
            if native && own {
                closed = ctx.input(|i| i.viewport().close_requested());
                placed = ctx.input(|i| {
                    let v = i.viewport();
                    let inner = v.inner_rect?;
                    let min = v.outer_rect.map_or(pos2(window.rect[0], window.rect[1]), |outer| outer.min);
                    Some([min.x, min.y, inner.width(), inner.height()])
                });
            }
        });
        if closed {
            self.ui.panel_windows.retain(|w| !f.panels.contains(&w.panel));
            ctx.data_mut(|d| d.remove::<[f32; 4]>(spawn_id));
            return actions;
        }
        // Remember where the window is, so the workspace reopens it there.
        if let Some(rect) = placed.map(PanelWindow::sanitized)
            && rect != window.rect
            && let Some(w) = self.ui.panel_windows.iter_mut().find(|w| w.panel == window.panel)
        {
            w.rect = rect;
        }
        actions
    }

    fn floating_group(&mut self, ctx: &egui::Context, index: usize, seqs: &dock::SeqTabs, actions: &mut Vec<DockAction>) -> Option<DropTarget<PanelKind>> {
        let floating = self.ui.floating.get(index)?.clone();
        let panel = *floating.panels.first()?;
        let rect = floating_rect(&floating, ctx.content_rect());
        let t = self.tokens;
        let id = egui::Id::new(("floating-panel", panel.id()));
        let mut output = None;
        let mut detach: Option<(egui::Pos2, egui::Vec2)> = None;
        egui::Area::new(id).order(egui::Order::Middle).fixed_pos(rect.min).constrain(false).interactable(true).show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
            let compact = floating.panels.len() == 1 && panel.compact();
            let header = if compact { 8.0 } else { t.tab_h };
            let strip = Rect::from_min_size(rect.min, vec2(rect.width(), header));
            let response = ui.interact(strip, id.with("move"), Sense::drag());
            self.auto.add_response(&format!("panel.float.{index}.move"), &response, "Move floating panel");
            let mut next = if response.dragged() { rect.translate(response.drag_delta()) } else { rect };
            // releasing the header off the main window leaves the panel on the desktop
            if response.drag_stopped()
                && let Some(at) = ctx.pointer_latest_pos().or_else(|| response.interact_pointer_pos())
                && !ctx.content_rect().expand(6.0).contains(at)
            {
                detach = Some((at, rect.size()));
            }
            let group = Group {
                path: format!("f{index}"),
                rect,
                content: Rect::from_min_max(pos2(rect.min.x, rect.min.y + header), rect.max),
                panels: floating.panels.clone(),
                active: floating.active,
            };
            let chrome = dock::draw_group_chrome_details(ui, &group, seqs, &t, &mut self.auto);
            actions.extend(chrome.actions);
            if let Some(active) = group.panels.get(group.active).copied() {
                self.panel_body(ui, active, group.content);
            }
            ui.painter().rect_stroke(rect, t.radius, Stroke::new(1.0, t.focus), StrokeKind::Outside);
            let grip = Rect::from_min_max(rect.max - vec2(14.0, 14.0), rect.max);
            let response = ui.interact(grip, id.with("resize"), Sense::click_and_drag());
            response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, response.enabled(), tl!("Resize floating panel")));
            self.auto.add_response(&format!("panel.float.{index}.resize"), &response, "Resize floating panel");
            for step in 0..3 {
                let offset = 4.0 + step as f32 * 4.0;
                ui.painter()
                    .line_segment([pos2(rect.max.x - offset, rect.max.y - 2.0), pos2(rect.max.x - 2.0, rect.max.y - offset)], Stroke::new(1.0, t.text_faint));
            }
            if response.hovered() || response.dragged() {
                ctx.set_cursor_icon(egui::CursorIcon::ResizeNwSe);
            }
            let delta = floating_resize_delta(ui, &response);
            if response.has_focus() {
                ui.painter().rect_stroke(grip, 2.0, Stroke::new(1.5, t.focus), StrokeKind::Inside);
            }
            if delta != egui::Vec2::ZERO {
                next = Rect::from_min_size(next.min, (next.size() + delta).max(vec2(160.0, 100.0)));
            }
            if next != rect && detach.is_none() {
                let _ = self.dock_command(ctx, Action::MoveFloating { panel, rect: [next.min.x, next.min.y, next.width(), next.height()] });
            }
            dock::paint_group_focus(ui, &group, self.ui.focused, &t);
            output = target(&group, chrome.tabs, false);
        });
        if let Some((at, size)) = detach {
            let origin = ctx.input_for(egui::ViewportId::ROOT, |i| i.viewport().inner_rect.map(|r| r.min)).unwrap_or(pos2(0.0, 0.0));
            let size = vec2(size.x.max(dock::PANEL_WINDOW_MIN[0]), size.y.max(dock::PANEL_WINDOW_MIN[1]));
            if self.detach_panel(ctx, panel, [origin.x + at.x - 40.0, origin.y + at.y - 10.0, size.x, size.y]).is_ok() {
                self.ui.focused = panel;
            }
            return None;
        }
        output
    }

    fn panel_drag(&mut self, ctx: &egui::Context, body: Rect, targets: &[DropTarget<PanelKind>]) {
        let Some(panel) = ctx.data(|data| data.get_temp::<PanelKind>(drag_id())) else { return };
        let canceled =
            ctx.input(|input| input.key_pressed(egui::Key::Escape) || !input.focused || (!input.pointer.primary_down() && !input.pointer.primary_released()));
        if canceled || (!self.ui.dock.contains(panel) && !self.ui.floating.iter().any(|group| group.panels.contains(&panel))) {
            ctx.data_mut(|data| data.remove::<PanelKind>(drag_id()));
            return;
        }
        let Some(pos) = ctx.pointer_latest_pos() else { return };
        let force_float = ctx.input(|input| input.modifiers.command);
        let edge = (!force_float).then(|| workspace_edge(body, pos, targets)).flatten();
        let proposal = (!force_float && edge.is_none()).then(|| resolve_drop(&panel, pos, targets)).flatten();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("dock-drop")));
        let t = self.tokens;
        if let Some((_, _, preview)) = edge {
            painter.rect_filled(preview.shrink(2.0), t.radius, t.focus.gamma_multiply(0.2));
            painter.rect_stroke(preview.shrink(2.0), t.radius, Stroke::new(2.0, t.focus), StrokeKind::Inside);
        }
        if let Some(proposal) = &proposal {
            painter.rect_filled(proposal.preview.shrink(2.0), t.radius, t.focus.gamma_multiply(0.2));
            painter.rect_stroke(proposal.preview.shrink(2.0), t.radius, Stroke::new(2.0, t.focus), StrokeKind::Inside);
            if let Some(mark) = proposal.insertion {
                painter.rect_filled(mark, 0.0, t.focus);
            }
        }
        let title = painter.layout_no_wrap(panel.title().to_owned(), crate::theme::Tokens::ui(12.0), Color32::WHITE);
        let ghost = Rect::from_min_size(pos + vec2(12.0, 10.0), title.size() + vec2(16.0, 8.0));
        painter.rect_filled(ghost, 4.0, t.focus.gamma_multiply(0.85));
        painter.galley(ghost.min + vec2(8.0, 4.0), title, Color32::WHITE);
        ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        ctx.request_repaint();
        if ctx.input(|input| input.pointer.primary_released()) {
            ctx.data_mut(|data| data.remove::<PanelKind>(drag_id()));
            // dropped off the main window: the panel lands on the desktop (another display)
            if !ctx.content_rect().contains(pos) {
                let origin = ctx.input_for(egui::ViewportId::ROOT, |i| i.viewport().inner_rect.map(|r| r.min)).unwrap_or(pos2(0.0, 0.0));
                if self.detach_panel(ctx, panel, [origin.x + pos.x - 40.0, origin.y + pos.y - 12.0, 420.0, 320.0]).is_ok() {
                    self.ui.focused = panel;
                }
                return;
            }
            if let Some((_, zone, _)) = edge {
                if crate::menus::invoke(self, ctx, "window.dockPanel", json!({"panel": panel.id(), "zone": zone})).is_ok() {
                    self.ui.focused = panel;
                }
                return;
            }
            let action = match proposal {
                Some(proposal) => Some(Action::Move { panel, anchor: proposal.anchor, placement: proposal.placement }),
                None if force_float || !targets.iter().any(|target| target.rect.contains(pos)) => {
                    Some(Action::Float { panel, rect: [pos.x - 40.0, pos.y - 12.0, 420.0, 320.0] })
                }
                None => None,
            };
            if let Some(action) = action
                && self.dock_command(ctx, action).is_ok()
            {
                self.ui.focused = panel;
            }
        }
    }
}

/// Width of the band along the workspace border that docks beside the whole layout.
const EDGE_BAND: f32 = 8.0;

/// The outermost drop zone: near the workspace border a drop splits the root, not the group
/// under the pointer. A floating group over the border (it rejects splits) keeps the drop.
fn workspace_edge(body: Rect, pos: egui::Pos2, targets: &[DropTarget<PanelKind>]) -> Option<(Zone, &'static str, Rect)> {
    if !body.is_finite() || !body.contains(pos) || targets.iter().any(|target| !target.allow_splits && target.rect.contains(pos)) {
        return None;
    }
    let band = EDGE_BAND.min(body.width() / 4.0).min(body.height() / 4.0);
    let edges = [
        (pos.x - body.left(), Zone::Left, "left"),
        (body.right() - pos.x, Zone::Right, "right"),
        (pos.y - body.top(), Zone::Top, "top"),
        (body.bottom() - pos.y, Zone::Bottom, "bottom"),
    ];
    let (distance, zone, name) = edges.into_iter().min_by(|a, b| a.0.total_cmp(&b.0))?;
    if distance > band {
        return None;
    }
    let size = body.size() * 0.25;
    let preview = match zone {
        Zone::Left => Rect::from_min_size(body.min, vec2(size.x, body.height())),
        Zone::Right => Rect::from_min_max(pos2(body.right() - size.x, body.top()), body.max),
        Zone::Top => Rect::from_min_size(body.min, vec2(body.width(), size.y)),
        Zone::Bottom | Zone::Center => Rect::from_min_max(pos2(body.left(), body.bottom() - size.y), body.max),
    };
    Some((zone, name, preview))
}

fn target(group: &Group, mut tabs: Vec<(PanelKind, Rect)>, allow_splits: bool) -> Option<DropTarget<PanelKind>> {
    let strip = Rect::from_min_max(group.rect.min, pos2(group.rect.max.x, group.content.min.y));
    // A compact grip still identifies the source group to the shared resolver.
    for panel in &group.panels {
        if !tabs.iter().any(|(candidate, _)| candidate == panel) {
            tabs.push((*panel, Rect::NOTHING));
        }
    }
    Some(DropTarget { anchor: *group.panels.get(group.active)?, rect: group.rect, tab_strip: Some(strip), tabs, allow_splits, accepts_tabs: true })
}

/// A corner is a two-axis resize control; focused arrows belong to it and Tab moves on.
fn floating_resize_delta(ui: &mut egui::Ui, response: &egui::Response) -> egui::Vec2 {
    if response.drag_started() {
        response.request_focus();
    }
    let mut delta = if response.dragged() { response.drag_delta() } else { egui::Vec2::ZERO };
    if response.has_focus() {
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(response.id, egui::EventFilter { horizontal_arrows: true, vertical_arrows: true, ..Default::default() })
        });
        ui.input_mut(|input| {
            for (key, amount) in [
                (egui::Key::ArrowLeft, vec2(-8.0, 0.0)),
                (egui::Key::ArrowRight, vec2(8.0, 0.0)),
                (egui::Key::ArrowUp, vec2(0.0, -8.0)),
                (egui::Key::ArrowDown, vec2(0.0, 8.0)),
            ] {
                if input.consume_key(egui::Modifiers::NONE, key) {
                    delta += amount;
                }
            }
        });
    }
    if response.enabled() { delta } else { egui::Vec2::ZERO }
}
