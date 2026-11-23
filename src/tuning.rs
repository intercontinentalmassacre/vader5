use crate::NAMES;
use crate::device::monitor::{Input, Monitor, Motor};
use eframe::egui::{self, RichText};

pub fn editable_byte(i: usize) -> bool {
    matches!(i,13..=84 | 110 | 117 | 124..=129 | 131..=154 | 160 | 174 | 791..=799 | 801 | 803..=811 | 813)
}

// Firmware samples the response at nine radii. Preserve the two curve points
// while moving the endpoints, including the extrapolated sample below centre.
fn stick_table(inner: u8, outer: u8, points: &[u8]) -> [u8; 9] {
    let low = f64::from(inner);
    let high = 100. - f64::from(outer);
    let point = |x, y| {
        (
            low + (high - low) * f64::from(x) / 127.,
            f64::from(y) * 100. / 127.,
        )
    };
    let nodes = [
        (low, 0.),
        point(points[0], points[1]),
        point(points[2], points[3]),
        (high, 100.),
    ];
    std::array::from_fn(|i| {
        let x = i as f64 * 12.5;
        let segment = if x <= nodes[1].0 {
            0
        } else if x <= nodes[2].0 {
            1
        } else {
            2
        };
        let (a, b) = (nodes[segment], nodes[segment + 1]);
        let y = if a.0 == b.0 {
            if x < a.0 { a.1 } else { b.1 }
        } else {
            a.1 + (b.1 - a.1) * (x - a.0) / (b.0 - a.0)
        };
        ((y + y.signum() * 1e-10).clamp(-50., 100.).trunc() as i16 + 50) as u8
    })
}

fn stick_response(travel: f32, table: &[u8]) -> f32 {
    let position = travel.clamp(0., 1.) * 8.;
    let index = (position as usize).min(7);
    let a = f32::from(table[index]);
    let b = f32::from(table[index + 1]);
    ((a + (b - a) * (position - index as f32) - 50.) / 100.).clamp(0., 1.)
}

// The trigger report already includes the saved travel window. At either
// limit several physical positions produce the same report; retain that range.
fn trigger_preview(value: u8, saved: [u8; 2], draft: [u8; 2]) -> (f32, f32) {
    let [start, end] = saved.map(f32::from);
    let travel = start + (end - start) * f32::from(value) / 255.;
    let [lo, hi] = draft.map(f32::from);
    let output = |x: f32| ((x - lo) / (hi - lo)).clamp(0., 1.);
    (
        output(if value == 0 { 0. } else { travel }),
        output(if value == 255 { 255. } else { travel }),
    )
}

fn trigger_meters(ui: &mut egui::Ui, current: Option<f32>, preview: Option<(f32, f32)>) {
    ui.columns(2, |cols| {
        for (ui, (title, color, reading)) in cols.iter_mut().zip([
            ("Текущее", crate::PRESSED_TEXT, current.map(|v| (v, v))),
            ("Предпросмотр", crate::GREEN, preview),
        ]) {
            ui.vertical_centered(|ui| {
                ui.colored_label(color, title);
                let (bar, _) = ui.allocate_exact_size(egui::vec2(24., 96.), egui::Sense::hover());
                let p = ui.painter();
                p.rect_filled(bar, 3., egui::Color32::from_gray(52));
                if let Some((low, high)) = reading {
                    for (level, ink) in [(high, color.gamma_multiply(0.45)), (low, color)] {
                        if level > 0. {
                            let mut fill = bar;
                            fill.min.y = bar.bottom() - bar.height() * level.clamp(0., 1.);
                            p.rect_filled(fill, 2., ink);
                        }
                    }
                }
                for tick in 0..=4 {
                    let y = bar.bottom() - bar.height() * tick as f32 / 4.;
                    p.line_segment(
                        [
                            egui::pos2(bar.right() + 2., y),
                            egui::pos2(bar.right() + 7., y),
                        ],
                        egui::Stroke::new(1.0_f32, crate::MUTED),
                    );
                }
                ui.monospace(reading_percent(reading));
            });
        }
    });
}

fn reading_percent(reading: Option<(f32, f32)>) -> String {
    match reading {
        Some((lo, hi)) if hi - lo > f32::EPSILON => {
            format!("{:.1}–{:.1}%", lo * 100., hi * 100.)
        }
        Some((v, _)) => format!("{:.1}%", v * 100.),
        None => "—".into(),
    }
}

fn output_meter(ui: &mut egui::Ui, color: egui::Color32, reading: Option<(f32, f32)>) {
    ui.monospace(reading_percent(reading));
    let (bar, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 8.), egui::Sense::hover());
    ui.painter().rect_filled(bar, 3., crate::BG);
    if let Some((low, high)) = reading {
        for (v, fill) in [(high, color.gamma_multiply(0.45)), (low, color)] {
            if v > 0. {
                let mut segment = bar;
                segment.set_width(bar.width() * v);
                ui.painter().rect_filled(segment, 3., fill);
            }
        }
    }
}

fn stick_indicator(
    ui: &mut egui::Ui,
    axes: Option<[f32; 2]>,
    inner: u8,
    outer: u8,
    table: Option<&[u8]>,
) {
    let current = axes.map(|[x, y]| egui::vec2(x, y));
    let preview = current
        .zip(table)
        .map(|(v, t)| v.normalized() * stick_response(v.length(), t));
    ui.columns(2, |columns| {
        for (i, (title, color, value)) in [
            ("Текущее", crate::PRESSED_TEXT, current),
            ("Предпросмотр", crate::GREEN, preview),
        ]
        .into_iter()
        .enumerate()
        {
            let ui = &mut columns[i];
            ui.vertical_centered(|ui| {
                ui.colored_label(color, title);
                let size = ui.available_width().min(104.);
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::hover());
                let center = rect.center();
                let radius = size / 2. - 5.;
                let p = ui.painter();
                let guide = egui::Stroke::new(1.0_f32, egui::Color32::from_gray(92));
                p.circle_filled(center, radius, crate::BG);
                for axis in [egui::vec2(radius, 0.), egui::vec2(0., radius)] {
                    p.line_segment([center - axis, center + axis], guide);
                }
                p.circle_stroke(center, radius, guide);
                // Thresholds refer to input travel, not to the remapped output.
                if i == 0 && table.is_some() {
                    for boundary in [inner, 100u8.saturating_sub(outer)] {
                        if (1..100).contains(&boundary) {
                            p.circle_stroke(
                                center,
                                radius * f32::from(boundary) / 100.,
                                egui::Stroke::new(1.0_f32, crate::GREEN.gamma_multiply(0.65)),
                            );
                        }
                    }
                }
                if let Some(v) = value {
                    let pos = center + radius * egui::vec2(v.x, -v.y) / v.length().max(1.);
                    p.line_segment([center, pos], egui::Stroke::new(2.0_f32, color));
                    p.circle_filled(pos, 4., color);
                    ui.monospace(format!("X {:+.1}%  Y {:+.1}%", v.x * 100., v.y * 100.));
                } else {
                    ui.weak("Нет показаний");
                }
                output_meter(
                    ui,
                    color,
                    value.map(|v| {
                        let m = v.length().min(1.);
                        (m, m)
                    }),
                );
            });
        }
    });
}

pub fn zones(ui: &mut egui::Ui, bytes: &mut [u8; 840], saved: &[u8; 840], live: Option<&Input>) {
    ui.weak("Текущее — сигнал геймпада. Предпросмотр — отклик с выбранными настройками. Двигайте ползунки, удерживая стик или триггер.");
    for sticks in [true, false] {
        let draw = |ui: &mut egui::Ui, side: usize, bytes: &mut [u8; 840]| {
            if sticks {
                stick_zone(ui, side, bytes, live);
            } else {
                trigger_zone(ui, side, bytes, saved, live);
            }
        };
        ui.add_space(10.);
        if ui.available_width() >= 710. {
            ui.columns(2, |columns| {
                for (side, ui) in columns.iter_mut().enumerate() {
                    draw(ui, side, bytes);
                }
            });
        } else {
            for side in 0..2 {
                draw(ui, side, bytes);
                ui.add_space(8.);
            }
        }
        if sticks {
            ui.add_space(6.);
            ui.separator();
        }
    }
}
fn stick_zone(ui: &mut egui::Ui, side: usize, bytes: &mut [u8; 840], live: Option<&Input>) {
    ui.label(
        RichText::new(["Левый стик · LX / LY", "Правый стик · RX / RY"][side])
            .size(18.)
            .strong(),
    );
    let core = 109 + side * 7;
    let bank = 790 + side * 12;
    let mut inner = bytes[core + 1];
    let mut outer = bytes[bank + 11];
    let axes = live.map(|i| [i.axes[side * 2], i.axes[side * 2 + 1]]);
    if inner == 127 {
        stick_indicator(ui, axes, inner, outer, None);
        ui.weak("Этот стик в профиле назначен на другой выход.");
        return;
    }
    if outer == 255 {
        outer = 0;
    }
    let points = &bytes[core + 2..core + 6];
    if u16::from(inner) + u16::from(outer) >= 100
        || points.iter().any(|&v| v > 127)
        || points[0] > points[2]
    {
        stick_indicator(ui, axes, inner, outer, None);
        ui.weak("Нестандартная кривая профиля.");
        return;
    }
    let center = ui
        .add(
            egui::Slider::new(&mut inner, 0..=99 - outer)
                .suffix("%")
                .text("Внутренняя зона"),
        )
        .changed();
    let edge = ui
        .add(
            egui::Slider::new(&mut outer, 0..=99 - inner)
                .suffix("%")
                .text("Внешняя зона"),
        )
        .changed();
    if center || edge {
        let table = stick_table(inner, outer, points);
        bytes[core + 1] = inner;
        bytes[bank + 11] = outer;
        bytes[bank + 1..bank + 10].copy_from_slice(&table);
    }
    stick_indicator(ui, axes, inner, outer, Some(&bytes[bank + 1..bank + 10]));
}
fn trigger_zone(
    ui: &mut egui::Ui,
    side: usize,
    bytes: &mut [u8; 840],
    saved: &[u8; 840],
    live: Option<&Input>,
) {
    ui.label(
        RichText::new(["Левый триггер · LT", "Правый триггер · RT"][side])
            .size(18.)
            .strong(),
    );
    let value = live.map(|input| input.triggers[side]);
    let current = value.map(|v| f32::from(v) / 255.);
    let base = 123 + side * 7;
    let mut start = bytes[base + 1];
    let mut end = bytes[base + 6];
    let travel_percent =
        |x: f64, _: std::ops::RangeInclusive<usize>| format!("{:.1}%", x * 100. / 255.);
    if start >= end {
        trigger_meters(ui, current, None);
        ui.weak("Нестандартный диапазон хода профиля.");
        return;
    }
    let first = ui
        .add(
            egui::Slider::new(&mut start, 0..=end - 1)
                .custom_formatter(travel_percent)
                .custom_parser(|s| {
                    s.trim()
                        .trim_end_matches('%')
                        .trim()
                        .replace(',', ".")
                        .parse::<f64>()
                        .ok()
                        .map(|v| v * 255. / 100.)
                })
                .text("Начало срабатывания"),
        )
        .changed();
    let last = ui
        .add(
            egui::Slider::new(&mut end, start + 1..=255)
                .custom_formatter(travel_percent)
                .custom_parser(|s| {
                    s.trim()
                        .trim_end_matches('%')
                        .trim()
                        .replace(',', ".")
                        .parse::<f64>()
                        .ok()
                        .map(|v| v * 255. / 100.)
                })
                .text("Полное нажатие"),
        )
        .changed();
    if first {
        bytes[base + 1..base + 4].fill(start);
    }
    if last {
        bytes[base + 4..base + 7].fill(end);
    }
    let preview =
        value.map(|v| trigger_preview(v, [saved[base + 1], saved[base + 6]], [start, end]));
    trigger_meters(ui, current, preview);
    ui.weak(match value {
        None => "Нет показаний",
        Some(0) => "В покое",
        Some(255) => "Полное нажатие",
        Some(_) => "Срабатывает",
    });
    if preview.is_some_and(|(low, high)| high - low > f32::EPSILON) {
        ui.weak("За границей записанной зоны точный ход не виден.");
    }
}

fn toggle(ui: &mut egui::Ui, byte: &mut u8, off: u8, label: &str) -> bool {
    let mut on = *byte == 0;
    if ui.checkbox(&mut on, label).changed() {
        *byte = if on { 0 } else { off };
    }
    on
}

fn percent(ui: &mut egui::Ui, label: &str, byte: &mut u8, min: u8) {
    let mut value = (*byte).clamp(min, 100);
    if ui
        .add(
            egui::Slider::new(&mut value, min..=100)
                .suffix("%")
                .text(label),
        )
        .changed()
    {
        *byte = value;
    }
}

fn target(bytes: &mut [u8; 840], next: u8) {
    let old = bytes[137];
    bytes[137] = next;
    if next == 0 {
        return;
    }
    bytes[143] = u8::from(next == 1);
    if next == 3 {
        bytes[138] = 255;
        bytes[140] = 0;
        bytes[144] = 255;
    } else if old == 3 && bytes[138] == 255 {
        bytes[138] = 12;
        bytes[139] = 1;
    }
    if old == 0 {
        // A factory profile stores zero in the unused secondary switch (D-pad Up).
        if bytes[144] == 0 {
            bytes[144] = 255;
        }
    }
}

fn activation_key(ui: &mut egui::Ui, label: &str, key: &mut u8) {
    let current = if *key == 255 {
        "Без кнопки".to_owned()
    } else {
        NAMES
            .get(*key as usize)
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("Кнопка {}", *key))
    };
    egui::ComboBox::from_label(label)
        .selected_text(current)
        .show_ui(ui, |ui| {
            ui.selectable_value(key, 255, "Без кнопки");
            for id in [
                12, 13, 10, 11, 22, 23, 18, 19, 20, 21, 16, 17, 14, 15, 4, 5, 7, 8, 0, 1, 2, 3,
            ] {
                ui.selectable_value(key, id, NAMES[id as usize]);
            }
        });
}

#[derive(Default)]
pub struct GyroTest {
    active: bool,
    peak: [f32; 3],
}
impl GyroTest {
    fn sample(&mut self, r: [f32; 3]) {
        if self.active {
            for (peak, value) in self.peak.iter_mut().zip(r) {
                *peak = peak.max(value.abs());
            }
            if self.peak.iter().all(|p| *p >= 25.) {
                self.active = false;
            }
        }
    }
    fn show(&mut self, ui: &mut egui::Ui, reading: Option<[f32; 3]>) {
        ui.label(RichText::new("Тест гироскопа").size(19.).strong());
        let Some(r) = reading else {
            self.active = false;
            ui.weak("Нет данных датчика");
            return;
        };
        self.sample(r);
        ui.label("Поверните геймпад по очереди вокруг каждой оси быстрее 25 °/с.");
        let count = self.peak.iter().filter(|p| **p >= 25.).count();
        ui.horizontal(|ui| {
            if self.active {
                if ui.button("Остановить тест").clicked() {
                    self.active = false;
                }
            } else if ui.button("Начать тест").clicked() {
                self.active = true;
                self.peak = [0.; 3];
            }
            ui.label(format!("Движение по осям: {count} из 3"));
        });
        for (i, axis) in ["X", "Y", "Z"].into_iter().enumerate() {
            ui.horizontal(|ui| {
                ui.monospace(format!("{axis}  {:+6.0} °/с", r[i]));
                let (rect, _) = ui.allocate_exact_size(egui::vec2(110., 8.), egui::Sense::hover());
                ui.painter().rect_filled(rect, 3., crate::BG);
                let mid = rect.center();
                ui.painter().vline(
                    mid.x,
                    rect.y_range(),
                    egui::Stroke::new(1.0_f32, crate::MUTED),
                );
                let end = mid + egui::vec2((r[i] / 180.).clamp(-1., 1.) * rect.width() / 2., 0.);
                ui.painter()
                    .line_segment([mid, end], egui::Stroke::new(5.0_f32, crate::GREEN));
                ui.colored_label(
                    if self.peak[i] >= 25. {
                        crate::GREEN
                    } else {
                        crate::MUTED
                    },
                    format!("Пик {:.0} °/с", self.peak[i]),
                );
            });
        }
    }
}
pub fn gyro(ui: &mut egui::Ui, b: &mut [u8; 840], reading: Option<[f32; 3]>, test: &mut GyroTest) {
    ui.label(RichText::new("Управление наклоном").size(19.).strong());
    let mut selected = b[137];
    let names = ["Выключено", "Левый стик", "Правый стик", "Мышь"];
    egui::ComboBox::from_label("Выход гироскопа")
        .selected_text(
            names
                .get(selected as usize)
                .unwrap_or(&"Из профиля")
                .to_string(),
        )
        .show_ui(ui, |ui| {
            for (idx, name) in names.iter().enumerate() {
                ui.selectable_value(&mut selected, idx as u8, *name);
            }
        });
    if selected != b[137] {
        target(b, selected);
    }
    if (1..=3).contains(&selected) {
        ui.add_space(10.);
        if selected != 3 {
            activation_key(ui, "Кнопка активации", &mut b[138]);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut b[139], 1, "Пока удерживается");
                if ui.selectable_value(&mut b[139], 0, "По нажатию").changed() {
                    b[144] = 255;
                }
            });
            if b[139] == 1 {
                activation_key(ui, "Вторая кнопка активации", &mut b[144]);
            }
        }
        ui.add_space(12.);
        let mut sensitivity = b[141].max(b[142]).min(100);
        if ui
            .add(
                egui::Slider::new(&mut sensitivity, 0..=100)
                    .suffix("%")
                    .text("Чувствительность"),
            )
            .changed()
        {
            b[141..143].fill(sensitivity);
        }
        if selected != 3 {
            percent(ui, "Мёртвая зона", &mut b[140], 0);
        }
    }
    ui.add_space(18.);
    ui.separator();
    ui.add_space(10.);
    test.show(ui, reading);
}

pub fn vibration(ui: &mut egui::Ui, b: &mut [u8; 840], monitor: Option<&Monitor>) {
    ui.label(RichText::new("Проверка моторов").size(19.).strong());
    ui.weak("Прямой импульс на 0,5 секунды. Каждый мотор запускается отдельно.");
    let pulse = monitor.and_then(Monitor::pulse);
    ui.horizontal_wrapped(|ui| {
        for (name, motor) in [
            ("Левая рукоятка", Motor::Left),
            ("Правая рукоятка", Motor::Right),
            ("Левый триггер", Motor::LeftTrigger),
            ("Правый триггер", Motor::RightTrigger),
        ] {
            let playing = pulse.is_some_and(|(m, _)| m == motor);
            let text = if playing {
                format!("{name} · тест")
            } else {
                name.into()
            };
            if ui
                .add_enabled(
                    monitor.is_some() && pulse.is_none(),
                    egui::Button::new(text),
                )
                .clicked()
                && let Some(m) = monitor
            {
                m.test(motor);
            }
        }
    });
    ui.add_space(12.);
    ui.separator();
    ui.add_space(10.);
    ui.label(RichText::new("Рукоятки").size(19.).strong());
    if toggle(ui, &mut b[145], 255, "Вибрация рукояток") {
        for (start, label) in [(146, "Левый мотор"), (150, "Правый мотор")] {
            ui.add_space(8.);
            if toggle(ui, &mut b[start], 255, label) {
                percent(ui, "Усиление", &mut b[start + 3], 0);
                let mut low = b[start + 1].min(b[start + 2]);
                let mut high = b[start + 1].max(b[start + 2]);
                let mut edited = ui
                    .add(egui::Slider::new(&mut low, 0..=high).text("Минимальная сила"))
                    .changed();
                edited |= ui
                    .add(egui::Slider::new(&mut high, low..=255).text("Максимальная сила"))
                    .changed();
                if edited {
                    b[start + 1..start + 3].copy_from_slice(&[low, high]);
                }
            }
        }
    }
    ui.add_space(14.);
    ui.separator();
    ui.add_space(10.);
    ui.label(RichText::new("Триггеры").size(19.).strong());
    if toggle(ui, &mut b[154], 1, "Вибрация триггеров") {
        percent(ui, "Левый триггер", &mut b[160], 1);
        percent(ui, "Правый триггер", &mut b[174], 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn local_preview_uses_draft_curve_and_inverts_saved_trigger_range() {
        let t = stick_table(25, 25, &[63, 63, 127, 127]);
        assert_eq!(stick_response(0.125, &t), 0.);
        assert_eq!(stick_response(0.5, &t), 0.5);
        assert_eq!(stick_response(0.875, &t), 1.);
        assert_eq!(trigger_preview(0, [50, 200], [50, 200]), (0., 0.));
        assert_eq!(trigger_preview(255, [50, 200], [50, 200]), (1., 1.));
        assert_eq!(trigger_preview(0, [50, 200], [0, 100]), (0., 0.5));
        assert_eq!(trigger_preview(255, [50, 200], [150, 250]), (0.5, 1.));
        let (low, high) = trigger_preview(85, [60, 180], [0, 200]);
        assert_eq!((low, high), (0.5, 0.5));
    }
    #[test]
    fn linear_stick_response_and_dead_zones() {
        let points = [63, 63, 127, 127];
        assert_eq!(
            stick_table(0, 0, &points),
            [50, 62, 75, 87, 100, 112, 125, 137, 150]
        );
        let table = stick_table(25, 25, &points);
        assert_eq!((table[2], table[4], table[6]), (50, 100, 150));
    }
    #[test]
    fn gyro_test_needs_motion_on_each_axis() {
        let mut t = GyroTest {
            active: true,
            peak: [0.; 3],
        };
        t.sample([-31., 0., 1.]);
        t.sample([1., 28., 7.]);
        assert!(t.active);
        t.sample([0., 0., -35.]);
        assert!(!t.active);
        assert_eq!(t.peak, [31., 28., 35.]);
    }
    #[test]
    fn enabling_aim_preserves_other_fields() {
        let mut b = [42; 840];
        b[137] = 0;
        b[144] = 0;
        let old = b;
        target(&mut b, 2);
        assert_eq!((b[137], b[143], b[144]), (2, 0, 255));
        for i in 0..840 {
            if !matches!(i, 137 | 143 | 144) {
                assert_eq!(b[i], old[i]);
            }
        }
        target(&mut b, 1);
        assert_eq!(b[143], 1);
    }
    #[test]
    fn mouse_uses_its_own_activation_format() {
        let mut b = [22; 840];
        target(&mut b, 3);
        assert_eq!([b[138], b[140], b[143], b[144]], [255, 0, 0, 255]);
        target(&mut b, 2);
        assert_eq!([b[138], b[139]], [12, 1]);
    }
    #[test]
    fn reserved_motor_bytes_are_protected() {
        for i in 155..183 {
            assert_eq!(editable_byte(i), matches!(i, 160 | 174));
        }
        assert!(!editable_byte(225));
        assert!(!editable_byte(130));
    }
}
