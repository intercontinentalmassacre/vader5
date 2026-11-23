use super::{BG, GREEN, NAMES, binding};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, TextureHandle, Vec2};

// Control centers in silhouette pixels, transformed together with the artwork.
struct View {
    title: &'static str,
    art: &'static [u8],
    crop: [f32; 4],
    keys: &'static [(usize, f32, f32)],
}
const VIEWS: [View; 3] = [
    View {
        title: "Спереди",
        art: include_bytes!("../assets/front.png"),
        crop: [140., 0., 1634., 887.],
        keys: &[
            (4, 1251., 364.),
            (5, 1341., 273.),
            (7, 1157., 273.),
            (8, 1250., 179.),
            (16, 1287., 515.),
            (17, 1377., 433.),
            (14, 505., 271.),
            (15, 1050., 497.),
            (6, 754., 253.),
            (9, 1021., 254.),
            (0, 695., 404.),
            (1, 786., 491.),
            (2, 695., 574.),
            (3, 606., 491.),
        ],
    },
    View {
        title: "Сзади",
        art: include_bytes!("../assets/rear.png"),
        crop: [0., 0., 1448., 1086.],
        keys: &[
            (18, 421., 525.),
            (19, 1027., 525.),
            (20, 438., 670.),
            (21, 1007., 670.),
        ],
    },
    View {
        title: "Сверху",
        art: include_bytes!("../assets/top.png"),
        crop: [0., 0., 1774., 887.],
        keys: &[
            (11, 478., 116.),
            (10, 1295., 116.),
            (23, 550., 274.),
            (22, 1224., 274.),
            (13, 337., 354.),
            (12, 1436., 354.),
        ],
    },
];

pub struct Map {
    pub input: super::Input,
    view: usize,
    key: usize,
    textures: [TextureHandle; 3],
}

impl Map {
    pub fn new(ctx: &egui::Context) -> Self {
        let textures = VIEWS.each_ref().map(|v| {
            let im = image::load_from_memory(v.art)
                .expect("embedded controller art")
                .to_rgba8();
            let size = [im.width() as usize, im.height() as usize];
            ctx.load_texture(
                v.title,
                egui::ColorImage::from_rgba_unmultiplied(size, im.as_raw()),
                egui::TextureOptions::LINEAR,
            )
        });
        Self {
            input: super::Input::default(),
            view: 0,
            key: 4,
            textures,
        }
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        draft: &mut [u8; 840],
        editable: bool,
        saved: Option<&[u8; 840]>,
        art_height: f32,
    ) {
        ui.horizontal(|ui| {
            for (i, v) in VIEWS.iter().enumerate() {
                if ui.selectable_label(self.view == i, v.title).clicked() && self.view != i {
                    self.view = i;
                    self.key = v.keys[0].0;
                }
            }
        });
        ui.add_space(8.);
        if ui.available_width() >= 730. {
            let width = ui.available_width() - 222. - ui.spacing().item_spacing.x;
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    Vec2::new(width, 0.),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        self.diagram(ui, draft, saved.is_some(), art_height);
                    },
                );
                ui.vertical(|ui| self.inspector(ui, draft, saved, editable));
            });
        } else {
            self.diagram(ui, draft, saved.is_some(), art_height);
            ui.add_space(10.);
            self.inspector(ui, draft, saved, editable);
        }
    }

    fn inspector(
        &self,
        ui: &mut egui::Ui,
        draft: &mut [u8; 840],
        saved: Option<&[u8; 840]>,
        editable: bool,
    ) {
        egui::Frame::new()
            .fill(Color32::from_rgb(32, 38, 35))
            .corner_radius(5)
            .inner_margin(14)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                ui.colored_label(
                    super::MUTED,
                    ["Лицевая панель", "Задняя панель", "Верхняя грань"][self.view],
                );
                ui.heading(NAMES[self.key]);
                ui.add_space(10.);
                if let Some(saved) = saved {
                    ui.label("Назначение");
                    ui.add_enabled_ui(editable, |ui| super::binding_menu(ui, draft, self.key));
                    let start = super::offset(self.key);
                    let field = start..start + 3;
                    if draft[field.clone()] != saved[field.clone()] {
                        ui.add_space(8.);
                        ui.colored_label(
                            super::MUTED,
                            format!("В профиле: {}", binding(saved, self.key)),
                        );
                        if ui
                            .add_enabled(editable, egui::Button::new("Отменить изменение"))
                            .clicked()
                        {
                            draft[field.clone()].copy_from_slice(&saved[field]);
                        }
                    }
                } else {
                    ui.label("Прочитайте профиль, чтобы изменить назначение.");
                }
            });
    }

    fn diagram(&mut self, ui: &mut egui::Ui, draft: &[u8; 840], loaded: bool, art_height: f32) {
        let view = &VIEWS[self.view];
        let texture = &self.textures[self.view];
        let [left, top, right, bottom] = view.crop;
        let extent = Vec2::new(right - left, bottom - top);
        let scale = (ui.available_width() / extent.x).min(art_height / extent.y);
        let (panel, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), extent.y * scale),
            Sense::hover(),
        );
        let rect = Rect::from_center_size(panel.center(), extent * scale);
        let tex = texture.size_vec2();
        let uv = Rect::from_min_max(
            Pos2::new(left / tex.x, top / tex.y),
            Pos2::new(right / tex.x, bottom / tex.y),
        );
        ui.painter()
            .rect_filled(panel, 5., Color32::from_rgb(32, 38, 35));
        ui.painter().image(texture.id(), rect, uv, Color32::WHITE);
        for &(key, x, y) in view.keys {
            let p = rect.min + Vec2::new(x - left, y - top) * scale;
            let short = match key {
                0 => "↑",
                1 => "→",
                2 => "↓",
                3 => "←",
                _ => NAMES[key],
            };
            let width = if short.len() > 3 { 38. } else { 25. };
            let marker = Rect::from_center_size(p, Vec2::new(width, 24.));
            let response = ui.interact(marker, ui.id().with(("art-key", key)), Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::Button,
                    true,
                    self.key == key,
                    NAMES[key],
                )
            });
            if response.clicked() {
                self.key = key;
            }
            let pressed = self.input.pressed(key);
            let signal = Color32::from_rgb(108, 213, 255);
            if matches!(key, 14 | 15) {
                let i = (key - 14) * 2;
                let delta = Vec2::new(self.input.axes[i], -self.input.axes[i + 1]);
                if delta.length() > 0.035 {
                    let tip = p + delta * 95. * scale;
                    ui.painter()
                        .line_segment([p, tip], Stroke::new(3.0_f32, signal));
                    ui.painter().circle_filled(tip, 5., signal);
                }
            }
            let active = self.key == key;
            let outline = if response.hovered() || response.has_focus() || active {
                GREEN
            } else {
                Color32::WHITE
            };
            ui.painter().rect(
                marker,
                12.,
                if pressed {
                    signal
                } else if active {
                    GREEN
                } else {
                    BG
                },
                Stroke::new(1.0_f32, outline),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                p,
                Align2::CENTER_CENTER,
                short,
                FontId::monospace(13.),
                if active || pressed {
                    BG
                } else {
                    Color32::WHITE
                },
            );
            response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_text(if loaded {
                    format!("{} → {}", NAMES[key], binding(draft, key))
                } else {
                    NAMES[key].into()
                });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_editable_key_has_one_visible_hotspot() {
        let mut seen = [0; 24];
        for v in VIEWS {
            let im = image::load_from_memory(v.art).unwrap();
            let [l, t, r, b] = v.crop;
            assert!(l >= 0. && t >= 0. && r <= im.width() as f32 && b <= im.height() as f32);
            for &(key, x, y) in v.keys {
                assert!(x > l && x < r && y > t && y < b);
                seen[key] += 1;
            }
        }
        assert_eq!(seen, [1; 24]);
    }
}
