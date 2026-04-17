//! Screen layout visualization view - enhanced version similar to ShareMouse

use eframe::egui;
use std::collections::HashMap;
use uuid::Uuid;

/// Screen layout view state
pub struct LayoutView {
    /// Screen rectangles for visualization
    screens: HashMap<String, ScreenRect>,

    /// Currently selected screen
    selected_screen: Option<String>,

    /// Drag state
    drag_state: Option<DragState>,

    /// Show alignment guide
    show_guides: bool,

    /// Show connections
    show_connections: bool,

    /// Grid snap enabled
    snap_to_grid: bool,

    /// Grid size
    grid_size: f32,

    /// Editing mode
    edit_mode: EditMode,

    /// Hovered screen
    hovered_screen: Option<String>,

    /// Hovered edge
    hovered_edge: Option<EdgeInfo>,
}

/// Screen rectangle for layout
#[derive(Debug, Clone)]
struct ScreenRect {
    id: String,
    name: String,
    hostname: String,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    is_local: bool,
    online: bool,
    /// Which device is in each direction (by edge direction)
    neighbors: [Option<String>; 4],
}

/// Edge direction
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
enum EdgeDirection {
    Left = 0,
    Top = 1,
    Right = 2,
    Bottom = 3,
}

impl EdgeDirection {
    fn all() -> &'static [EdgeDirection] {
        &[EdgeDirection::Left, EdgeDirection::Top, EdgeDirection::Right, EdgeDirection::Bottom]
    }

    fn name(&self) -> &'static str {
        match self {
            EdgeDirection::Left => "Left",
            EdgeDirection::Top => "Top",
            EdgeDirection::Right => "Right",
            EdgeDirection::Bottom => "Bottom",
        }
    }

    fn opposite(&self) -> EdgeDirection {
        match self {
            EdgeDirection::Left => EdgeDirection::Right,
            EdgeDirection::Top => EdgeDirection::Bottom,
            EdgeDirection::Right => EdgeDirection::Left,
            EdgeDirection::Bottom => EdgeDirection::Top,
        }
    }
}

/// Edge hover information
#[derive(Debug, Clone)]
struct EdgeInfo {
    screen: String,
    direction: EdgeDirection,
}

/// Drag state for moving screens
#[derive(Debug, Clone)]
struct DragState {
    screen: String,
    start_x: f32,
    start_y: f32,
    offset_x: f32,
    offset_y: f32,
}

/// Edit mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditMode {
    /// Move screens around
    Arrange,
    /// Connect screens by clicking edges
    Connect,
}

impl LayoutView {
    /// Create a new layout view
    pub fn new() -> Self {
        let mut screens = HashMap::new();

        // Add local screen
        let mut local_screen = ScreenRect {
            id: "local".to_string(),
            name: "This PC".to_string(),
            hostname: "Your-Computer".to_string(),
            x: 300.0,
            y: 200.0,
            width: 240.0,
            height: 160.0,
            is_local: true,
            online: true,
            neighbors: [None, None, None, None],
        };
        screens.insert("local".to_string(), local_screen);

        Self {
            screens,
            selected_screen: None,
            drag_state: None,
            show_guides: true,
            show_connections: true,
            snap_to_grid: true,
            grid_size: 20.0,
            edit_mode: EditMode::Arrange,
            hovered_screen: None,
            hovered_edge: None,
        }
    }

    /// Add or update a device screen
    pub fn add_device(&mut self, id: String, name: String, hostname: String, online: bool) {
        // Generate position based on existing screens
        let x = 50.0 + (self.screens.len() as f32 * 180.0) % 600.0;
        let y = 50.0 + (self.screens.len() as f32 * 150.0) % 400.0;

        let screen = ScreenRect {
            id: id.clone(),
            name,
            hostname,
            x,
            y,
            width: 200.0,
            height: 140.0,
            is_local: false,
            online,
            neighbors: [None, None, None, None],
        };

        self.screens.insert(id, screen);
    }

    /// Remove a device screen
    pub fn remove_device(&mut self, id: &str) {
        if let Some(screen) = self.screens.remove(id) {
            // Remove connections to this screen
            for (_, other_screen) in self.screens.iter_mut() {
                for neighbor in &mut other_screen.neighbors {
                    if neighbor.as_ref() == Some(&screen.id) {
                        *neighbor = None;
                    }
                }
            }
        }
    }

    /// Show the layout view
    pub fn show(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // Header
        ui.vertical(|ui| {
            ui.heading("⚙ Screen Layout");
            ui.label("Arrange screens to define how your mouse moves between devices.");
            ui.add_space(5.0);
        });

        ui.separator();
        ui.add_space(10.0);

        // Top toolbar
        ui.horizontal(|ui| {
            // Mode selection
            ui.label("Mode:");
            ui.selectable_value(&mut self.edit_mode, EditMode::Arrange, "📐 Arrange");
            ui.selectable_value(&mut self.edit_mode, EditMode::Connect, "🔗 Connect");

            ui.separator();

            // View options
            ui.checkbox(&mut self.show_guides, "Grid");
            ui.checkbox(&mut self.show_connections, "Connections");
            ui.checkbox(&mut self.snap_to_grid, "Snap");

            ui.separator();

            // Actions
            if ui.button("Auto Arrange").clicked() {
                self.auto_arrange();
            }
            if ui.button("Reset").clicked() {
                *self = Self::new();
            }
        });

        ui.add_space(10.0);

        // Main layout area
        let desired_size = egui::vec2(ui.available_width(), 450.0);

        egui::CentralPanel::default()
            .show_inside(ui, |ui| {
                let response = ui.allocate_response(desired_size, egui::Sense::click_and_drag());

                // Draw background
                self.draw_background(ui, response.rect);

                // Calculate connections to draw
                let connections = self.calculate_connections();

                // Draw connections (under screens)
                if self.show_connections {
                    self.draw_connections(ui, response.rect, &connections);
                }

                // Draw edge controls
                if self.edit_mode == EditMode::Connect {
                    self.draw_edge_controls(ui, response.rect);
                }

                // Draw screens
                self.draw_screens(ui, response.rect, &response);

                // Handle interactions
                self.handle_interactions(&response);

                // Draw tooltip
                self.draw_tooltip(ui, response.rect);
            });

        // Bottom panel - device properties
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(10.0);

        self.show_properties_panel(ui);
    }

    /// Draw background with grid
    fn draw_background(&self, ui: &mut egui::Ui, rect: egui::Rect) {
        // Fill background
        ui.painter().rect_filled(
            rect,
            0.0,
            egui::Color32::from_rgb(25, 25, 30),
        );

        // Draw grid
        if self.show_guides {
            let grid_size = self.grid_size;
            let color = egui::Color32::from_rgba_premultiplied(255, 255, 255, 10);
            let stroke = egui::Stroke::new(1.0, color);

            // Vertical lines
            let start_x = (rect.min.x / grid_size).floor() as i32;
            let end_x = (rect.max.x / grid_size).ceil() as i32;
            for x in start_x..=end_x {
                let x = x as f32 * grid_size;
                ui.painter().line_segment(
                    [egui::pos2(x, rect.min.y), egui::pos2(x, rect.max.y)],
                    stroke,
                );
            }

            // Horizontal lines
            let start_y = (rect.min.y / grid_size).floor() as i32;
            let end_y = (rect.max.y / grid_size).ceil() as i32;
            for y in start_y..=end_y {
                let y = y as f32 * grid_size;
                ui.painter().line_segment(
                    [egui::pos2(rect.min.x, y), egui::pos2(rect.max.x, y)],
                    stroke,
                );
            }
        }
    }

    /// Draw connection arrows between screens
    fn draw_connections(&self, ui: &mut egui::Ui, canvas_rect: egui::Rect, connections: &[(EdgeInfo, EdgeInfo)]) {
        for (from, to) in connections {
            if let (Some(from_screen), Some(to_screen)) = (
                self.screens.get(&from.screen),
                self.screens.get(&to.screen),
            ) {
                let from_rect = self.screen_rect(canvas_rect, from_screen);
                let to_rect = self.screen_rect(canvas_rect, to_screen);

                // Calculate connection points
                let from_point = self.edge_center(&from_rect, from.direction);
                let to_point = self.edge_center(&to_rect, to.direction);

                // Draw arrow
                self.draw_arrow(ui, from_point, to_point);
            }
        }
    }

    /// Draw an arrow between two points
    fn draw_arrow(&self, ui: &mut egui::Ui, from: egui::Pos2, to: egui::Pos2) {
        let color = egui::Color32::from_rgb(100, 200, 100);
        let stroke = egui::Stroke::new(2.0, color);

        // Line
        ui.painter().line_segment([from, to], stroke);

        // Arrowhead using a small circle
        ui.painter().circle_filled(to, 4.0, color);
    }

    /// Draw screens
    fn draw_screens(&mut self, ui: &mut egui::Ui, canvas_rect: egui::Rect, response: &egui::Response) {
        self.hovered_screen = None;

        for (id, screen) in self.screens.iter() {
            let rect = self.screen_rect(canvas_rect, screen);

            // Check hover
            if response.hovered() {
                if let Some(pos) = response.hover_pos() {
                    if rect.contains(pos) {
                        self.hovered_screen = Some(id.clone());
                    }
                }
            }

            // Determine colors
            let is_selected = self.selected_screen.as_ref() == Some(id);
            let is_hovered = self.hovered_screen.as_ref() == Some(id);

            let (base_color, border_color, border_width) = if is_selected {
                (egui::Color32::from_rgb(60, 120, 200), egui::Color32::from_rgb(100, 180, 255), 3.0)
            } else if is_hovered {
                (egui::Color32::from_rgb(70, 110, 170), egui::Color32::from_rgb(150, 200, 250), 2.0)
            } else if screen.is_local {
                (egui::Color32::from_rgb(50, 100, 160), egui::Color32::from_rgb(100, 150, 200), 2.0)
            } else if screen.online {
                (egui::Color32::from_rgb(45, 90, 140), egui::Color32::from_rgb(80, 130, 180), 2.0)
            } else {
                (egui::Color32::from_rgb(60, 60, 70), egui::Color32::from_rgb(100, 100, 110), 2.0)
            };

            // Draw screen rectangle with rounded corners
            let corner_radius = if self.edit_mode == EditMode::Connect { 4.0 } else { 8.0 };
            ui.painter().rect(
                rect,
                corner_radius,
                base_color,
                egui::Stroke::new(border_width, border_color),
            );

            // Inner highlight for selected
            if is_selected {
                ui.painter().rect_stroke(
                    rect.shrink(3.0),
                    corner_radius,
                    egui::Stroke::new(1.0, egui::Color32::from_rgba_premultiplied(255, 255, 255, 50)),
                );
            }

            // Device name
            let text_color = if screen.online {
                egui::Color32::WHITE
            } else {
                egui::Color32::GRAY
            };

            ui.painter().text(
                egui::pos2(rect.center().x, rect.min.y + 20.0),
                egui::Align2::CENTER_CENTER,
                &screen.name,
                egui::FontId::proportional(14.0),
                text_color,
            );

            // Hostname
            ui.painter().text(
                egui::pos2(rect.center().x, rect.min.y + 38.0),
                egui::Align2::CENTER_CENTER,
                &screen.hostname,
                egui::FontId::proportional(11.0),
                egui::Color32::from_gray(150),
            );

            // Resolution
            ui.painter().text(
                egui::pos2(rect.center().x, rect.center().y + 15.0),
                egui::Align2::CENTER_CENTER,
                format!("{}x{}", screen.width as i32, screen.height as i32),
                egui::FontId::proportional(12.0),
                egui::Color32::from_gray(130),
            );

            // Status indicator
            let status_color = if screen.online {
                egui::Color32::from_rgb(100, 200, 100)
            } else {
                egui::Color32::GRAY
            };
            let status_pos = egui::pos2(rect.min.x + 10.0, rect.min.y + 10.0);
            ui.painter().circle_filled(status_pos, 4.0, status_color);

            // Local indicator
            if screen.is_local {
                ui.painter().text(
                    egui::pos2(rect.max.x - 8.0, rect.max.y - 8.0),
                    egui::Align2::RIGHT_BOTTOM,
                    "🏠",
                    egui::FontId::proportional(12.0),
                    egui::Color32::from_rgba_premultiplied(255, 255, 255, 100),
                );
            }

            // Draw neighbor indicators in Connect mode
            if self.edit_mode == EditMode::Connect {
                self.draw_neighbor_indicators(ui, &rect, id);
            }
        }
    }

    /// Draw neighbor connection points
    fn draw_neighbor_indicators(&self, ui: &mut egui::Ui, rect: &egui::Rect, screen_id: &str) {
        if let Some(screen) = self.screens.get(screen_id) {
            for (idx, neighbor_id) in screen.neighbors.iter().enumerate() {
                let direction = match idx {
                    0 => EdgeDirection::Left,
                    1 => EdgeDirection::Top,
                    2 => EdgeDirection::Right,
                    3 => EdgeDirection::Bottom,
                    _ => continue,
                };

                let center = self.edge_center(rect, direction);
                let is_connected = neighbor_id.is_some();

                // Draw connection point
                let color = if is_connected {
                    egui::Color32::from_rgb(100, 200, 100)
                } else {
                    egui::Color32::from_gray(100)
                };

                ui.painter().circle_filled(center, 5.0, color);

                // Draw hover highlight
                if let Some(ref hovered) = self.hovered_edge {
                    if hovered.screen == screen_id && hovered.direction == direction {
                        ui.painter().circle_stroke(
                            center,
                            7.0,
                            egui::Stroke::new(2.0, egui::Color32::WHITE),
                        );
                    }
                }
            }
        }
    }

    /// Draw edge controls for connection editing
    fn draw_edge_controls(&mut self, ui: &mut egui::Ui, canvas_rect: egui::Rect) {
        self.hovered_edge = None;

        for (id, screen) in self.screens.iter() {
            let rect = self.screen_rect(canvas_rect, screen);

            for &direction in EdgeDirection::all() {
                let center = self.edge_center(&rect, direction);
                let hit_rect = egui::Rect::from_center_size(center, egui::vec2(12.0, 12.0));

                if let Some(pos) = ui.input(|i| i.pointer.hover_pos()) {
                    if hit_rect.contains(pos) {
                        self.hovered_edge = Some(EdgeInfo {
                            screen: id.clone(),
                            direction,
                        });
                    }
                }
            }
        }
    }

    /// Handle mouse interactions
    fn handle_interactions(&mut self, response: &egui::Response) {
        // Handle clicking
        if response.clicked() {
            if let Some(edge) = self.hovered_edge.clone() {
                if self.edit_mode == EditMode::Connect {
                    self.handle_edge_click(&edge);
                }
            } else if let Some(ref hovered) = self.hovered_screen {
                self.selected_screen = Some(hovered.clone());
            } else {
                self.selected_screen = None;
            }
        }

        // Handle dragging in Arrange mode
        if response.dragged() && self.edit_mode == EditMode::Arrange {
            if let Some(ref selected) = self.selected_screen {
                if let Some(screen) = self.screens.get_mut(selected) {
                    if self.drag_state.is_none() {
                        self.drag_state = Some(DragState {
                            screen: selected.clone(),
                            start_x: screen.x,
                            start_y: screen.y,
                            offset_x: 0.0,
                            offset_y: 0.0,
                        });
                    }

                    if let Some(ref drag) = self.drag_state {
                        let delta = response.drag_delta();
                        screen.x += delta.x;
                        screen.y += delta.y;

                        // Snap to grid
                        if self.snap_to_grid {
                            screen.x = (screen.x / self.grid_size).round() * self.grid_size;
                            screen.y = (screen.y / self.grid_size).round() * self.grid_size;
                        }
                    }
                }
            }
        } else {
            self.drag_state = None;
        }
    }

    /// Handle edge click for connection editing
    fn handle_edge_click(&mut self, clicked_edge: &EdgeInfo) {
        if let Some(selected) = self.selected_screen.clone() {
            if selected != clicked_edge.screen {
                // Connect selected screen's clicked edge to the clicked edge
                // For simplicity, we'll connect the nearest edge
                self.connect_screens(&selected, &clicked_edge.screen);
            }
        }
        self.selected_screen = Some(clicked_edge.screen.clone());
    }

    /// Connect two screens (find nearest edges and connect them)
    fn connect_screens(&mut self, from: &str, to: &str) {
        // Get positions first
        let (from_x, from_y, to_x, to_y) = {
            let screens = self.screens.clone();
            let from_pos = screens.get(from).map(|s| (s.x, s.y));
            let to_pos = screens.get(to).map(|s| (s.x, s.y));
            match (from_pos, to_pos) {
                (Some((fx, fy)), Some((tx, ty))) => (fx, fy, tx, ty),
                _ => return,
            }
        };

        // Simple auto-connect based on relative positions
        let dx = to_x - from_x;
        let dy = to_y - from_y;

        let (from_edge, to_edge) = if dx.abs() > dy.abs() {
            if dx > 0.0 {
                (EdgeDirection::Right, EdgeDirection::Left)
            } else {
                (EdgeDirection::Left, EdgeDirection::Right)
            }
        } else {
            if dy > 0.0 {
                (EdgeDirection::Bottom, EdgeDirection::Top)
            } else {
                (EdgeDirection::Top, EdgeDirection::Bottom)
            }
        };

        // Update connections one at a time to avoid borrow issues
        if let Some(from_screen) = self.screens.get_mut(from) {
            from_screen.neighbors[from_edge as usize] = Some(to.to_string());
        }
        if let Some(to_screen) = self.screens.get_mut(to) {
            to_screen.neighbors[to_edge as usize] = Some(from.to_string());
        }
    }

    /// Draw tooltip
    fn draw_tooltip(&self, ui: &mut egui::Ui, canvas_rect: egui::Rect) {
        if let Some(ref edge) = self.hovered_edge {
            if self.edit_mode == EditMode::Connect {
                if let Some(screen) = self.screens.get(&edge.screen) {
                    let rect = self.screen_rect(canvas_rect, screen);
                    let center = self.edge_center(&rect, edge.direction);

                    let _tooltip = format!("{} → Click to connect", edge.direction.name());
                    let pos = egui::pos2(center.x + 15.0, center.y - 15.0);

                    ui.painter().rect(
                        egui::Rect::from_center_size(pos, egui::vec2(80.0, 20.0)),
                        3.0,
                        egui::Color32::BLACK,
                        egui::Stroke::new(1.0, egui::Color32::WHITE),
                    );
                }
            }
        }
    }

    /// Show properties panel for selected screen
    fn show_properties_panel(&self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            if let Some(ref selected) = self.selected_screen {
                if let Some(screen) = self.screens.get(selected) {
                    ui.horizontal(|ui| {
                        ui.heading(format!("📺 {}", screen.name));
                        if screen.is_local {
                            ui.label("(This PC)");
                        }
                        ui.separator();
                        ui.label(format!("Position: ({:.0}, {:.0})", screen.x, screen.y));
                    });

                    ui.add_space(5.0);

                    // Neighbor information
                    ui.label("Connections:");
                    ui.indent("connections", |ui| {
                        for direction in EdgeDirection::all() {
                            if let Some(neighbor_id) = &screen.neighbors[*direction as usize] {
                                if let Some(neighbor) = self.screens.get(neighbor_id) {
                                    ui.label(format!(
                                        "{} → {}",
                                        direction.name(),
                                        neighbor.name
                                    ));
                                }
                            }
                        }
                        if screen.neighbors.iter().all(|n| n.is_none()) {
                            ui.label("No connections. Click edge indicators in Connect mode to add.");
                        }
                    });
                }
            } else {
                ui.label("👆 Select a screen to view its properties");
                ui.label("💡 Use Arrange mode to move screens, Connect mode to link them");
            }
        });
    }

    /// Get screen rectangle relative to canvas
    fn screen_rect(&self, canvas_rect: egui::Rect, screen: &ScreenRect) -> egui::Rect {
        egui::Rect::from_min_size(
            canvas_rect.min + egui::vec2(screen.x, screen.y),
            egui::vec2(screen.width, screen.height)
        )
    }

    /// Get center point of an edge
    fn edge_center(&self, rect: &egui::Rect, direction: EdgeDirection) -> egui::Pos2 {
        match direction {
            EdgeDirection::Left => egui::pos2(rect.min.x, rect.center().y),
            EdgeDirection::Top => egui::pos2(rect.center().x, rect.min.y),
            EdgeDirection::Right => egui::pos2(rect.max.x, rect.center().y),
            EdgeDirection::Bottom => egui::pos2(rect.center().x, rect.max.y),
        }
    }

    /// Calculate all connections to draw
    fn calculate_connections(&self) -> Vec<(EdgeInfo, EdgeInfo)> {
        let mut connections = Vec::new();

        for (id, screen) in self.screens.iter() {
            for (idx, neighbor_id) in screen.neighbors.iter().enumerate() {
                if let Some(neighbor) = neighbor_id {
                    let direction = match idx {
                        0 => EdgeDirection::Left,
                        1 => EdgeDirection::Top,
                        2 => EdgeDirection::Right,
                        3 => EdgeDirection::Bottom,
                        _ => continue,
                    };

                    // Only add each connection once
                    if id < neighbor {
                        connections.push((
                            EdgeInfo { screen: id.clone(), direction },
                            EdgeInfo { screen: neighbor.clone(), direction: direction.opposite() },
                        ));
                    }
                }
            }
        }

        connections
    }

    /// Auto-arrange screens in a grid
    fn auto_arrange(&mut self) {
        let mut screens: Vec<_> = self.screens.values().cloned().collect();

        // Sort: local screen first, then by name
        screens.sort_by(|a, b| {
            if a.is_local && !b.is_local {
                return std::cmp::Ordering::Less;
            }
            if !a.is_local && b.is_local {
                return std::cmp::Ordering::Greater;
            }
            a.name.cmp(&b.name)
        });

        let mut x = 100.0;
        let mut y = 150.0;
        let spacing = 30.0;
        let max_width = 700.0;

        for screen in screens {
            if let Some(s) = self.screens.get_mut(&screen.id) {
                s.x = x;
                s.y = y;

                x += s.width + spacing;
                if x > max_width {
                    x = 100.0;
                    y += s.height + spacing;
                }
            }
        }
    }
}

impl Default for LayoutView {
    fn default() -> Self {
        Self::new()
    }
}
