use crate::{GREEN, MUTED};
use eframe::egui::{self, Color32, RichText};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Effect {
    Steady,
    Breath,
    Double,
    Beacon,
    Wave,
    Scanner,
    Kitt,
    Spark,
    Off,
}
const EFFECTS: [Effect; 9] = [
    Effect::Steady,
    Effect::Breath,
    Effect::Double,
    Effect::Beacon,
    Effect::Wave,
    Effect::Scanner,
    Effect::Kitt,
    Effect::Spark,
    Effect::Off,
];
const SPEED: [u8; 10] = [50, 36, 26, 18, 13, 9, 6, 4, 2, 1];

impl Effect {
    fn name(self) -> &'static str {
        match self {
            Self::Steady => "Постоянный цвет",
            Self::Breath => "Дыхание",
            Self::Double => "Двойной импульс",
            Self::Beacon => "Маяк",
            Self::Wave => "Бегущая волна",
            Self::Scanner => "Сканер",
            Self::Kitt => "КИТТ",
            Self::Spark => "Искры",
            Self::Off => "Выключено",
        }
    }
    // Only the frame table encodes custom animations. The type byte is a firmware category.
    fn layout(self) -> (u8, usize) {
        match self {
            Self::Steady => (5, 1),
            Self::Breath => (2, 2),
            Self::Double => (2, 7),
            Self::Beacon => (2, 5),
            Self::Wave => (1, 10),
            Self::Scanner => (1, 9),
            Self::Kitt => (3, 10),
            Self::Spark => (1, 8),
            Self::Off => (6, 1),
        }
    }
    fn level(self, f: usize, z: usize) -> u8 {
        match self {
            Self::Steady => 255,
            Self::Off => 0,
            Self::Breath => [255, 0][f],
            Self::Double => [255, 0, 215, 0, 0, 0, 0][f],
            Self::Beacon => [255, 75, 0, 0, 0][f],
            Self::Wave => [255, 170, 88, 30, 5, 0, 0, 0, 0, 0][(10 + f - z) % 10],
            Self::Scanner => {
                [255, 105, 18, 0, 0, 0, 0, 0, 0, 0][z.abs_diff([0, 2, 4, 6, 8, 9, 7, 5, 2][f])]
            }
            Self::Kitt => {
                let head: usize = [0, 2, 4, 6, 8, 9, 7, 5, 3, 1][f];
                let tail = if (1..=5).contains(&f) {
                    head - 1
                } else {
                    head + 1
                };
                if z == head {
                    255
                } else if z == tail {
                    18
                } else {
                    0
                }
            }
            Self::Spark => match (z * 7 + f * 3 + z * f) % 13 {
                0 | 3 => 255,
                5 => 65,
                _ => 0,
            },
        }
    }
    fn write(self, b: &mut [u8; 320], rgb: [u8; 3]) {
        let (kind, count) = self.layout();
        b[2..5].copy_from_slice(&[0, 0, (count - 1) as u8]);
        b[8] = kind;
        b[9] = 0;
        b[20..].fill(0);
        for (i, pixel) in b[20..20 + 30 * count].chunks_exact_mut(3).enumerate() {
            let level = u16::from(self.level(i / 10, i % 10));
            pixel.copy_from_slice(&rgb.map(|v| (u16::from(v) * level / 255) as u8));
        }
    }
}
fn base_color(b: &[u8; 320]) -> [u8; 3] {
    let c = b[20..]
        .chunks_exact(3)
        .max_by_key(|p| p.iter().map(|v| u16::from(*v)).sum::<u16>())
        .unwrap();
    [c[0], c[1], c[2]]
}
fn identify(b: &[u8; 320]) -> Option<Effect> {
    let effect = EFFECTS
        .into_iter()
        .find(|e| e.layout() == (b[8], usize::from(b[4]) + 1))?;
    let mut expected = *b;
    effect.write(&mut expected, base_color(b));
    (expected == *b).then_some(effect)
}
fn preview_phase(seconds: f64, speed_byte: u8) -> f64 {
    seconds / (0.025 * f64::from(speed_byte.max(1)))
}

fn preview(ui: &mut egui::Ui, b: &[u8; 320]) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 74.), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, 6., Color32::from_rgb(21, 24, 23));
    let start = usize::from(b[3].min(9));
    let last = usize::from(b[4].min(9)).max(start);
    let count = last - start + 1;
    let tick = preview_phase(ui.input(|i| i.time), b[5]);
    let index = tick as usize % count;
    let fraction = tick.fract() as f32;
    let bar = rect.shrink2(egui::vec2(18., 23.));
    for z in 0..10 {
        let color_at = |f: usize, ch: usize| f32::from(b[20 + 30 * f + z * 3 + ch]);
        let rgb: [u8; 3] = std::array::from_fn(|ch| {
            let a = color_at(start + index, ch);
            let c = color_at(start + (index + 1) % count, ch);
            ((a + (c - a) * fraction) * f32::from(b[6].min(100)) / 100.) as u8
        });
        let step = bar.width() / 10.;
        let r = egui::Rect::from_min_size(
            bar.min + egui::vec2(z as f32 * step, 0.),
            egui::vec2(step - 3., bar.height()),
        );
        painter.rect_filled(r, 3., Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
    }
    if count > 1 && b[6] > 0 {
        let refresh_ms = (u64::from(b[5].max(1)) * 25 / 2).clamp(16, 33);
        ui.ctx()
            .request_repaint_after(Duration::from_millis(refresh_ms));
    }
}
pub fn show(ui: &mut egui::Ui, draft: &mut [u8; 320], original: &[u8; 320]) {
    if original[..2] != [0, 3] || original[7] != 10 {
        ui.label("Этот формат подсветки пока не поддерживается.");
        return;
    }
    let current = identify(draft)
        .map(Effect::name)
        .unwrap_or(if draft[8] == 7 {
            "Заводская анимация"
        } else {
            "Из профиля"
        });
    ui.horizontal(|ui| {
        ui.label(RichText::new(current).size(20.).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.colored_label(MUTED, "Предпросмотр");
        });
    });
    preview(ui, draft);
    ui.add_space(16.);
    if ui.available_width() >= 720. {
        let width = ui.available_width() - 256. - ui.spacing().item_spacing.x;
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(width, 0.),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    gallery(ui, draft);
                },
            );
            ui.vertical(|ui| controls(ui, draft, original));
        });
    } else {
        gallery(ui, draft);
        ui.add_space(14.);
        controls(ui, draft, original);
    }
}

fn gallery(ui: &mut egui::Ui, draft: &mut [u8; 320]) {
    ui.label(RichText::new("Эффект").size(18.).strong());
    ui.add_space(5.);
    let mode = identify(draft);
    let columns = if ui.available_width() >= 455. { 3 } else { 2 };
    let width = (ui.available_width() - ui.spacing().item_spacing.x * (columns - 1) as f32)
        / columns as f32;
    for row in EFFECTS.chunks(columns) {
        ui.horizontal(|ui| {
            for &effect in row {
                let selected = mode == Some(effect);
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(width, 62.), egui::Sense::click());
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::Button,
                        true,
                        selected,
                        effect.name(),
                    )
                });
                let ink = if selected { GREEN } else { MUTED };
                let fill = if selected || response.hovered() {
                    Color32::from_rgb(46, 53, 47)
                } else {
                    Color32::from_rgb(35, 39, 37)
                };
                ui.painter().rect_filled(rect, 3., fill);
                if response.has_focus() || selected {
                    ui.painter().rect_stroke(
                        rect,
                        3.,
                        egui::Stroke::new(1.0_f32, GREEN),
                        egui::StrokeKind::Inside,
                    );
                }
                // Show spatial patterns as LEDs and pulse patterns as a rhythm over time.
                let count = effect.layout().1;
                let spatial = matches!(
                    effect,
                    Effect::Wave | Effect::Scanner | Effect::Kitt | Effect::Spark
                );
                let moving = spatial && (selected || response.hovered() || response.has_focus());
                let frame = if moving {
                    (ui.input(|i| i.time) * 5.) as usize % count
                } else {
                    count / 3
                };
                let step_width = (rect.width() - 24.) / 10.;
                for step in 0..10 {
                    let level = if spatial {
                        effect.level(frame, step)
                    } else {
                        effect.level(step * count / 10, 0)
                    };
                    let signal = if effect == Effect::Kitt {
                        Color32::from_rgb(255, 72, 60)
                    } else {
                        ink
                    };
                    let base = rect.min + egui::vec2(12. + step as f32 * step_width, 24.);
                    let bar_height = if spatial {
                        6.
                    } else {
                        2. + 12. * f32::from(level) / 255.
                    };
                    let color = if spatial {
                        Color32::from_rgb(61, 65, 62).lerp_to_gamma(signal, f32::from(level) / 255.)
                    } else {
                        signal
                    };
                    ui.painter().rect_filled(
                        egui::Rect::from_min_max(
                            base - egui::vec2(0., bar_height),
                            base + egui::vec2(step_width - 2., 0.),
                        ),
                        1.,
                        color,
                    );
                }
                if moving {
                    ui.ctx().request_repaint_after(Duration::from_millis(100));
                }
                ui.painter().text(
                    rect.left_bottom() + egui::vec2(12., -8.),
                    egui::Align2::LEFT_BOTTOM,
                    effect.name(),
                    egui::FontId::proportional(15.),
                    if selected { GREEN } else { crate::TEXT },
                );
                if response.clicked() && !selected {
                    let mut rgb = base_color(draft);
                    if effect == Effect::Kitt {
                        rgb = [255, 0, 0];
                    } else if rgb == [0; 3] {
                        rgb = [180, 223, 129];
                    }
                    effect.write(draft, rgb);
                }
            }
        });
    }
}
fn controls(ui: &mut egui::Ui, draft: &mut [u8; 320], original: &[u8; 320]) {
    let mode = identify(draft);
    ui.label(RichText::new("Параметры").size(18.).strong());
    ui.add_space(5.);
    egui::Frame::new()
        .fill(Color32::from_rgb(35, 39, 37))
        .corner_radius(3)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.spacing_mut().slider_width = (ui.available_width() - 168.).clamp(58., 180.);
            egui::Grid::new("light-settings")
                .min_col_width(64.)
                .spacing([6., 10.])
                .show(ui, |ui| {
                    if mode.is_some_and(|m| m != Effect::Off) {
                        ui.label("Цвет полосы");
                        ui.horizontal(|ui| {
                            let mut rgb = base_color(draft);
                            if ui.color_edit_button_srgb(&mut rgb).changed() {
                                mode.unwrap().write(draft, rgb);
                            }
                            ui.monospace(format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]));
                        });
                        ui.end_row();
                    }
                    ui.label("Яркость");
                    ui.add(egui::Slider::new(&mut draft[6], 0..=100).suffix("%"));
                    ui.end_row();
                    if draft[4] > draft[3] {
                        let mut speed = 1 + SPEED
                            .iter()
                            .enumerate()
                            .min_by_key(|(_, t)| t.abs_diff(draft[5]))
                            .unwrap()
                            .0;
                        ui.label("Скорость");
                        if ui.add(egui::Slider::new(&mut speed, 1..=10)).changed() {
                            draft[5] = SPEED[speed - 1];
                        }
                        ui.end_row();
                    }
                });
        });
    ui.add_space(8.);
    if ui
        .add_enabled(
            draft != original,
            egui::Button::new(RichText::new("Вернуть подсветку профиля").color(GREEN)).frame(false),
        )
        .clicked()
    {
        *draft = *original;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn each_speed_advances_the_preview_faster() {
        for pair in SPEED.windows(2) {
            assert!(preview_phase(1., pair[0]) < preview_phase(1., pair[1]));
        }
        assert_eq!(preview_phase(1., 0), preview_phase(1., 1));
    }
    #[test]
    #[ignore = "requires a controller; saves a temporary animation then restores the original"]
    fn hardware_animation_save_and_restore() -> Result<(), String> {
        use crate::device::{load, save};
        let before = load()?;
        let mut animation = before.lights;
        Effect::Breath.write(&mut animation, [41, 107, 213]);
        animation[5] = 15;
        animation[6] = 75;
        let written = save(&before, &before.bytes, &animation, false);
        if written.is_ok() {
            std::thread::sleep(std::time::Duration::from_secs(8));
        }
        let current = load()?;
        if current.slot != before.slot
            || current.bytes[..225] != before.bytes[..225]
            || current.bytes[227..] != before.bytes[227..]
        {
            return Err("Профиль изменился во время теста.".into());
        }
        let matches = current.lights == animation;
        let restored = save(&current, &current.bytes, &before.lights, false)?;
        assert_eq!(restored.lights, before.lights);
        written?;
        assert!(matches);
        Ok(())
    }
    #[test]
    fn effects_roundtrip_without_changing_brightness_or_speed() {
        for effect in EFFECTS {
            let mut b = [11; 320];
            effect.write(&mut b, [137, 17, 201]);
            assert_eq!(identify(&b), Some(effect));
            assert_eq!(&b[5..8], &[11; 3]);
            if effect.layout().1 > 1 {
                assert_ne!(&b[20..50], &b[50..80]);
            }
        }
    }
    #[test]
    fn foreign_animation_is_preserved() {
        let mut b = [0; 320];
        Effect::Wave.write(&mut b, [40, 80, 12]);
        b[25] ^= 1;
        assert_eq!(identify(&b), None);
    }
}
