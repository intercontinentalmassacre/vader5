mod device;
mod lighting;
mod tuning;
use device::monitor::{Battery, Input, Monitor};
#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Map,
    List,
    Lights,
    Zones,
    Gyro,
    Vibration,
}
impl Tab {
    const ALL: [Self; 6] = [
        Self::Map,
        Self::List,
        Self::Lights,
        Self::Zones,
        Self::Gyro,
        Self::Vibration,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::Map => "Схема",
            Self::List => "Назначения",
            Self::Lights => "Подсветка",
            Self::Zones => "Мёртвые зоны",
            Self::Gyro => "Гироскоп",
            Self::Vibration => "Вибрация",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Self::Map => "Схема геймпада",
            Self::List => "Назначения кнопок",
            _ => self.label(),
        }
    }
}
mod silhouettes;

use eframe::egui::{self, Color32, Frame, RichText};
use std::sync::{Arc, mpsc};

const NAMES: [&str; 24] = [
    "Крестовина ↑",
    "Крестовина →",
    "Крестовина ↓",
    "Крестовина ←",
    "A",
    "B",
    "View",
    "X",
    "Y",
    "Menu",
    "LB",
    "RB",
    "LT",
    "RT",
    "L3",
    "R3",
    "C",
    "Z",
    "M1",
    "M2",
    "M3",
    "M4",
    "LM",
    "RM",
];
const EXTRA: [usize; 8] = [18, 19, 20, 21, 22, 23, 16, 17];
const STANDARD: [usize; 16] = [4, 5, 7, 8, 10, 11, 12, 13, 14, 15, 0, 1, 2, 3, 6, 9];
const BG: Color32 = Color32::from_rgb(28, 31, 30);
const TEXT: Color32 = Color32::from_rgb(236, 239, 232);
const MUTED: Color32 = Color32::from_rgb(178, 183, 176);
const PRESSED_TEXT: Color32 = Color32::from_rgb(108, 213, 255);
const PRESSED_BG: Color32 = Color32::from_rgb(31, 67, 82);
const GREEN: Color32 = Color32::from_rgb(185, 232, 129);
fn offset(key: usize) -> usize {
    13 + key * 3
}

fn binding(bytes: &[u8; 840], key: usize) -> String {
    let i = offset(key);
    let mut name = match bytes[i] {
        0xff => "По умолчанию".to_owned(),
        0x20 => return "Макрос".to_owned(),
        0xfe => return "Клавиатура / мышь".to_owned(),
        id => NAMES
            .get(id as usize)
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("Назначение 0x{id:02X}")),
    };
    if bytes[i + 2] > 0 {
        name.push_str(&format!(" · турбо {}/с", bytes[i + 2]));
    }
    name
}

type Reply = Result<device::Profile, String>;
struct Editor {
    devices: Vec<device::Controller>,
    selected: Option<std::ffi::CString>,
    scan: Option<mpsc::Receiver<Result<Vec<device::Controller>, String>>>,
    next_scan: std::time::Instant,
    next_retry: std::time::Instant,
    profile: Option<Arc<device::Profile>>,
    draft: [u8; 840],
    work: Option<mpsc::Receiver<Reply>>,
    saving: bool,
    standard: bool,
    last_standard_press: u64,
    tab: Tab,
    lights: [u8; 320],
    monitor: Option<Monitor>,
    gyro_test: tuning::GyroTest,
    map: silhouettes::Map,
    message: String,
    error: bool,
}

fn desktop_font(ctx: &egui::Context) {
    #[cfg(windows)]
    let path =
        std::env::var_os("WINDIR").map(|p| std::path::PathBuf::from(p).join("Fonts/segoeui.ttf"));
    #[cfg(target_os = "linux")]
    let path = std::process::Command::new("fc-match")
        .args(["-f", "%{file}", "sans:lang=ru"])
        .output()
        .ok()
        .filter(|p| p.status.success())
        .and_then(|p| String::from_utf8(p.stdout).ok())
        .map(|p| std::path::PathBuf::from(p.trim()));
    if let Some(data) = path.and_then(|p| std::fs::read(p).ok()) {
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert("desktop".into(), egui::FontData::from_owned(data).into());
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "desktop".into());
        ctx.set_fonts(fonts);
    }
}

impl Editor {
    fn new(ctx: &egui::Context) -> Self {
        desktop_font(ctx);
        let mut style = (*ctx.style()).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = BG;
        style.visuals.override_text_color = Some(TEXT);
        style.visuals.selection.bg_fill = Color32::from_rgb(61, 80, 57);
        style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(44, 52, 48);
        style.spacing.interact_size.y = 32.;
        style.spacing.item_spacing = egui::vec2(8., 5.);
        style.visuals.widgets.noninteractive.bg_stroke.color = Color32::from_rgb(63, 68, 63);
        style.spacing.button_padding = egui::vec2(12., 5.);
        style
            .text_styles
            .insert(egui::TextStyle::Body, egui::FontId::proportional(15.));
        style
            .text_styles
            .insert(egui::TextStyle::Button, egui::FontId::proportional(15.));
        style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(44, 52, 48);
        ctx.set_style(style);
        Self {
            devices: Vec::new(),
            selected: None,
            scan: None,
            next_scan: std::time::Instant::now(),
            next_retry: std::time::Instant::now(),
            profile: None,
            draft: [0; 840],
            work: None,
            saving: false,
            standard: false,
            last_standard_press: 0,
            tab: Tab::Map,
            lights: [0; 320],
            monitor: None,
            gyro_test: Default::default(),
            map: silhouettes::Map::new(ctx),
            message: "Поиск геймпадов…".into(),
            error: false,
        }
    }

    fn discover(&mut self, ctx: &egui::Context, dirty: bool) {
        use std::time::{Duration, Instant};
        ctx.request_repaint_after(Duration::from_secs(2));
        if self.work.is_some() {
            return;
        }
        if let Some(rx) = &self.scan {
            let result = match rx.try_recv() {
                Ok(list) => list,
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => Err("Поиск геймпадов прерван.".into()),
            };
            self.scan = None;
            self.next_scan = Instant::now() + Duration::from_secs(2);
            let devices = match result {
                Ok(devices) => devices,
                Err(e) => {
                    self.message = e;
                    self.error = true;
                    return;
                }
            };
            self.devices = devices;
            if !self.online() {
                if dirty {
                    self.monitor.take();
                    return;
                }
                self.selected = None;
                self.profile = None;
                self.monitor.take();
                self.error = false;
                self.message = if self.devices.is_empty() {
                    "Геймпад не найден"
                } else {
                    "Выберите геймпад"
                }
                .into();
                if let [only] = self.devices.as_slice() {
                    self.selected = Some(only.path.clone());
                    self.run(ctx, false);
                }
            } else if Instant::now() >= self.next_retry {
                if !dirty
                    && (self.profile.is_none()
                        || self.monitor.as_ref().is_some_and(Monitor::finished))
                {
                    self.run(ctx, false);
                } else if dirty
                    && self.monitor.as_ref().is_none_or(Monitor::finished)
                    && let Some(p) = &self.profile
                {
                    self.monitor = Some(Monitor::start(p));
                    self.next_retry = Instant::now() + Duration::from_secs(8);
                }
            }
        } else if Instant::now() >= self.next_scan {
            let (tx, rx) = mpsc::channel();
            self.scan = Some(rx);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(device::discover());
                ctx.request_repaint();
            });
        }
    }

    fn online(&self) -> bool {
        self.devices
            .iter()
            .any(|d| Some(&d.path) == self.selected.as_ref())
    }

    fn run(&mut self, ctx: &egui::Context, save: bool) {
        let Some(path) = self.selected.clone() else {
            return;
        };
        self.next_retry = std::time::Instant::now() + std::time::Duration::from_secs(8);
        self.gyro_test = Default::default();
        let current = self.profile.clone();
        let draft = self.draft;
        let lights = self.lights;
        let retry = self.saving && self.error;
        let monitor = self.monitor.take();
        let ctx = ctx.clone();
        let (tx, rx) = mpsc::channel();
        self.work = Some(rx);
        self.saving = save;
        self.error = false;
        self.message = if save {
            "Запись профиля…"
        } else {
            "Чтение профиля…"
        }
        .into();
        std::thread::spawn(move || {
            drop(monitor);
            let result = if save {
                current
                    .ok_or_else(|| "Сначала прочитайте профиль".to_string())
                    .and_then(|p| device::save(&p, &draft, &lights, retry))
            } else {
                device::load_at(&path)
            };
            let _ = tx.send(result);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self) {
        let Some(rx) = &self.work else { return };
        let result = match rx.try_recv() {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("Связь прервана. Прочитайте профиль ещё раз.".into())
            }
        };
        self.work = None;
        match result {
            Ok(p) => {
                self.draft = p.bytes;
                self.lights = p.lights;
                self.monitor = Some(Monitor::start(&p));
                self.profile = Some(Arc::new(p));
                self.message = if self.saving {
                    "Запись проверена"
                } else {
                    "Профиль прочитан"
                }
                .into();
            }
            Err(e) => {
                self.message = e;
                self.error = true;
                if self.saving {
                    self.monitor = self.profile.as_ref().map(|p| Monitor::start(p));
                } else {
                    self.profile = None;
                }
            }
        }
    }
}

impl eframe::App for Editor {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.poll();

        let changed = self
            .profile
            .as_ref()
            .map(|p| {
                (0..24)
                    .filter(|&key| {
                        let i = offset(key);
                        p.bytes[i..i + 3] != self.draft[i..i + 3]
                    })
                    .count()
            })
            .unwrap_or(0);

        let light_dirty = self
            .profile
            .as_ref()
            .is_some_and(|p| p.lights != self.lights);
        let tuning_dirty = self.profile.as_ref().is_some_and(|p| {
            p.bytes[109..183] != self.draft[109..183] || p.bytes[790..814] != self.draft[790..814]
        });
        let dirty = changed > 0 || light_dirty || tuning_dirty;
        self.discover(ctx, dirty);
        let mut busy = self.work.is_some();
        if !busy && self.online() {
            if dirty && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::S)) {
                self.run(ctx, true);
                busy = true;
            } else if !dirty
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F5))
            {
                self.run(ctx, false);
                busy = true;
            }
        }
        let live = self.monitor.as_ref().map(Monitor::state);
        let input = live
            .as_ref()
            .and_then(|v| v.as_ref().ok())
            .copied()
            .unwrap_or_default();
        let (sequence, buttons) = input.standard_press;
        let mut scroll_buttons =
            if self.tab == Tab::List && self.standard && sequence != self.last_standard_press {
                buttons
            } else {
                0
            };
        self.last_standard_press = sequence;
        if self.monitor.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        egui::TopBottomPanel::bottom("actions")
            .frame(
                Frame::new()
                    .fill(Color32::from_rgb(32, 38, 35))
                    .inner_margin(16),
            )
            .show(ctx, |ui| {
                if dirty && !self.online() {
                    ui.colored_label(
                        Color32::from_rgb(255, 182, 150),
                        "Геймпад отключён. Подключите его снова или отмените правки.",
                    );
                    ui.add_space(6.);
                } else if self.error {
                    ui.colored_label(Color32::from_rgb(255, 182, 150), &self.message);
                    ui.add_space(6.);
                }
                ui.horizontal_wrapped(|ui| {
                    if busy {
                        ui.spinner();
                    }
                    let status = if busy {
                        self.message.clone()
                    } else if tuning_dirty {
                        "Есть изменения профиля".to_owned()
                    } else if light_dirty {
                        if changed == 0 {
                            "Подсветка изменена".to_owned()
                        } else {
                            format!("Подсветка и кнопки ({changed})")
                        }
                    } else if changed > 0 {
                        format!("Изменено кнопок: {changed}")
                    } else if self.error {
                        String::new()
                    } else {
                        self.message.clone()
                    };
                    if !status.is_empty() {
                        ui.label(RichText::new(status).color(MUTED));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let active = dirty && !busy && self.online();
                        let mut apply = egui::Button::new(
                            RichText::new("Записать в геймпад").color(if active {
                                BG
                            } else {
                                MUTED
                            }),
                        );
                        if active {
                            apply = apply.fill(GREEN);
                        }
                        if self.profile.is_some()
                            && ui
                                .add_enabled(active, apply)
                                .on_hover_text("Записать изменения (Ctrl+S)")
                                .clicked()
                        {
                            self.run(ctx, true);
                        }
                        if dirty
                            && ui
                                .add_enabled(!busy, egui::Button::new("Отменить"))
                                .clicked()
                        {
                            if let Some(p) = &self.profile {
                                self.draft = p.bytes;
                                self.lights = p.lights;
                            }
                            self.message = "Правки отменены".into();
                            self.error = false;
                        }
                    });
                });
            });
        let wide = ctx.available_rect().width() >= 850.;
        if wide {
            egui::SidePanel::left("navigation")
                .resizable(false)
                .exact_width(184.)
                .frame(
                    Frame::new()
                        .fill(Color32::from_rgb(20, 23, 22))
                        .inner_margin(16),
                )
                .show(ctx, |ui| {
                    ui.add_space(10.);
                    ui.label(RichText::new("Vader 5 Pro").size(24.).strong());
                    ui.add_space(26.);
                    navigation(ui, &mut self.tab, true);
                });
        }
        egui::CentralPanel::default()
            .frame(Frame::new().fill(BG).inner_margin(22))
            .show(ctx, |ui| {
                if !self.devices.is_empty() {
                    let mut picked = self.selected.clone();
                    let selected_label = self
                        .devices
                        .iter()
                        .find(|d| Some(&d.path) == picked.as_ref())
                        .map(|d| d.label.as_str());
                    if self.devices.len() == 1
                        && let Some(label) = selected_label
                    {
                        ui.label(label);
                    } else {
                        let selector = ui.add_enabled_ui(!busy && !dirty, |ui| {
                            egui::ComboBox::from_id_salt("controller")
                                .selected_text(selected_label.unwrap_or("Выберите геймпад"))
                                .show_ui(ui, |ui| {
                                    for d in &self.devices {
                                        ui.selectable_value(
                                            &mut picked,
                                            Some(d.path.clone()),
                                            &d.label,
                                        );
                                    }
                                });
                        });
                        if dirty {
                            selector
                                .response
                                .on_disabled_hover_text("Сначала запишите или отмените изменения.");
                        }
                    }
                    if picked != self.selected {
                        self.selected = picked;
                        self.profile = None;
                        self.run(ctx, false);
                        busy = true;
                    }
                }
                ui.horizontal_wrapped(|ui| {
                    if !wide {
                        navigation(ui, &mut self.tab, false);
                    }
                    if let Some(profile) = &self.profile {
                        ui.label(RichText::new(format!("Профиль {}", profile.slot + 1)).strong());
                        ui.separator();
                        battery_indicator(ui, input.battery);
                    }
                    if self.online()
                        && ui
                            .add_enabled(
                                !busy && !dirty,
                                egui::Button::new(if self.profile.is_some() {
                                    "Перечитать профиль"
                                } else {
                                    "Прочитать профиль"
                                }),
                            )
                            .on_hover_text("Загрузить активный профиль геймпада (F5)")
                            .clicked()
                    {
                        self.run(ctx, false);
                    }
                });
                ui.separator();
                ui.add_space(10.);
                ui.heading(RichText::new(self.tab.title()).size(26.));
                ui.add_space(12.);
                let monitor = self
                    .monitor
                    .as_ref()
                    .filter(|_| matches!(&live, Some(Ok(_))));
                if self.tab != Tab::Map
                    && let Some(Err(e)) = &live
                {
                    ui.colored_label(MUTED, e);
                }
                if matches!(self.tab, Tab::Zones | Tab::Gyro | Tab::Vibration) {
                    if let Some(profile) = &self.profile {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            ui.add_enabled_ui(!busy, |ui| {
                                if self.tab == Tab::Zones {
                                    tuning::zones(
                                        ui,
                                        &mut self.draft,
                                        &profile.bytes,
                                        live.as_ref().and_then(|v| v.as_ref().ok()),
                                    );
                                } else if self.tab == Tab::Gyro {
                                    tuning::gyro(
                                        ui,
                                        &mut self.draft,
                                        live.as_ref().and_then(|r| r.as_ref().ok()).map(|i| i.gyro),
                                        &mut self.gyro_test,
                                    );
                                } else {
                                    tuning::vibration(ui, &mut self.draft, monitor);
                                }
                            })
                        });
                    } else {
                        ui.label("Для настройки прочитайте профиль геймпада.");
                    }
                    return;
                }
                if self.tab == Tab::Lights {
                    if let Some(p) = &self.profile {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            ui.add_enabled_ui(!busy, |ui| {
                                lighting::show(ui, &mut self.lights, &p.lights)
                            })
                        });
                    } else {
                        ui.label("Для настройки подсветки прочитайте профиль.");
                    }
                    return;
                }
                if self.tab == Tab::Map {
                    match &live {
                        Some(Ok(i)) => {
                            ui.label(live_label(i));
                        }
                        Some(Err(e)) => {
                            ui.colored_label(MUTED, e);
                        }
                        None => {}
                    }
                    self.map.input = input;
                    let map_height = (ui.available_height() - 100.).clamp(270., 440.);
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let loaded = self.profile.is_some();
                        self.map.show(
                            ui,
                            &mut self.draft,
                            loaded && !busy,
                            self.profile.as_ref().map(|p| &p.bytes),
                            map_height,
                        );
                    });
                    return;
                }
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.standard, false, "Лепестки и доп. кнопки");
                    ui.selectable_value(&mut self.standard, true, "Основные кнопки");
                });
                ui.add_space(15.);
                if let Some(profile) = &self.profile {
                    egui::ScrollArea::vertical()
                        .id_salt(("bindings", self.standard))
                        .auto_shrink([false, false])
                        .show_viewport(ui, |ui, viewport| {
                            let visible = viewport.translate(ui.min_rect().min.to_vec2());
                            if self.standard {
                                let mut group = |ui: &mut egui::Ui, title: &str, keys: &[usize]| {
                                    ui.label(RichText::new(title).size(18.).strong());
                                    for &key in keys {
                                        let row = assignment_row(
                                            ui,
                                            key,
                                            &mut self.draft,
                                            &profile.bytes,
                                            !busy,
                                            &input,
                                        );
                                        if scroll_buttons & (1 << key) != 0
                                            && (row.top() < visible.top()
                                                || row.bottom() > visible.bottom())
                                            && !ui.memory(|m| m.any_popup_open())
                                        {
                                            ui.scroll_to_rect(row, None);
                                            scroll_buttons = 0;
                                        }
                                    }
                                };
                                let groups: [(&str, &[usize]); 4] = [
                                    ("Кнопки ABXY", &[4, 5, 7, 8]),
                                    ("Бамперы и триггеры", &[10, 11, 12, 13]),
                                    ("Крестовина", &[0, 1, 2, 3]),
                                    ("Стики и меню", &[14, 15, 6, 9]),
                                ];
                                if ui.available_width() >= 680. {
                                    for pair in groups.chunks_exact(2) {
                                        ui.columns(2, |columns| {
                                            for (column, &(title, keys)) in
                                                columns.iter_mut().zip(pair)
                                            {
                                                group(column, title, keys);
                                            }
                                        });
                                        ui.add_space(8.);
                                    }
                                } else {
                                    for (title, keys) in groups {
                                        group(ui, title, keys);
                                        ui.add_space(8.);
                                    }
                                }
                            } else {
                                ui.label(RichText::new("Задние лепестки").size(19.).strong());
                                ui.add_space(4.);
                                paddle_bindings(ui, &mut self.draft, &profile.bytes, !busy, &input);
                                ui.add_space(16.);
                                ui.label(RichText::new("Дополнительные кнопки").size(19.).strong());
                                ui.add_space(4.);

                                ui.horizontal(|ui| {
                                    source_column(ui, 20., |ui| {
                                        ui.colored_label(MUTED, "Кнопка");
                                    });
                                    ui.colored_label(MUTED, "Назначение");
                                });
                                for &key in &EXTRA[4..] {
                                    ui.separator();
                                    assignment_row(
                                        ui,
                                        key,
                                        &mut self.draft,
                                        &profile.bytes,
                                        !busy,
                                        &input,
                                    );
                                }
                            }
                        });
                } else if !busy && self.devices.is_empty() {
                    ui.label("Подключите геймпад кабелем или через USB-приёмник в режиме XInput.");
                } else if !busy && self.selected.is_none() {
                    ui.label("Выберите геймпад, чтобы загрузить его активный профиль.");
                }
            });
        if busy && ctx.input(|input| input.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
    }
}

fn battery_indicator(ui: &mut egui::Ui, battery: Option<Battery>) {
    let (text, color) = match battery {
        Some(Battery::Discharging(p)) if p <= 20 => (
            format!("Низкий заряд · {p}%"),
            Color32::from_rgb(255, 158, 122),
        ),
        Some(Battery::Discharging(p)) => (format!("Заряд {p}%"), MUTED),
        Some(Battery::Charging(p)) => (format!("Зарядка · {p}%"), GREEN),
        Some(Battery::Full) => ("Заряжен · 100%".into(), GREEN),
        None => ("Заряд —".into(), MUTED),
    };
    ui.colored_label(color, text)
        .on_hover_text("Уровень батареи по данным геймпада, шаг 20%.");
}

fn navigation(ui: &mut egui::Ui, tab: &mut Tab, vertical: bool) {
    if !vertical {
        egui::ComboBox::from_id_salt("section")
            .selected_text(tab.label())
            .width(135.)
            .show_ui(ui, |ui| {
                for section in Tab::ALL {
                    ui.selectable_value(tab, section, section.label());
                }
            })
            .response
            .on_hover_text("Выбрать раздел");
        return;
    }
    for destination in Tab::ALL {
        if matches!(destination, Tab::Map | Tab::Lights) {
            if destination == Tab::Lights {
                ui.add_space(16.);
            }
            ui.label(
                RichText::new(if destination == Tab::Map {
                    "Управление"
                } else {
                    "Устройство"
                })
                .size(13.)
                .color(MUTED),
            );
            ui.add_space(4.);
        }
        let selected = *tab == destination;
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 36.), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::Button,
                true,
                selected,
                destination.label(),
            )
        });
        let visuals = ui.style().interact_selectable(&response, selected);
        if selected || response.hovered() || response.has_focus() {
            ui.painter().rect_filled(rect, 4., visuals.weak_bg_fill);
        }
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect,
                4.,
                egui::Stroke::new(1.0_f32, GREEN),
                egui::StrokeKind::Inside,
            );
        }
        ui.painter().text(
            rect.left_center() + egui::vec2(10., 0.),
            egui::Align2::LEFT_CENTER,
            destination.label(),
            egui::FontId::proportional(16.),
            if selected { GREEN } else { TEXT },
        );
        if response.clicked() {
            *tab = destination;
        }
    }
}

fn live_label(input: &Input) -> String {
    let mut keys: Vec<_> = NAMES
        .iter()
        .enumerate()
        .filter(|(i, _)| input.pressed(*i))
        .map(|(_, name)| *name)
        .collect();
    for (code, name) in [(24, "Fn"), (25, "Turbo"), (27, "Home")] {
        if input.pressed(code) {
            keys.push(name);
        }
    }
    for (axis, name) in [(0, "Левый стик"), (2, "Правый стик")] {
        if input.axes[axis].hypot(input.axes[axis + 1]) > 0.07 {
            keys.push(name);
        }
    }
    if keys.is_empty() {
        "Ввод · ожидание нажатия".into()
    } else {
        format!("Ввод · {}", keys.join(" + "))
    }
}

fn paddle_bindings(
    ui: &mut egui::Ui,
    draft: &mut [u8; 840],
    saved: &[u8; 840],
    editable: bool,
    input: &Input,
) {
    // The order mirrors the rear view: M1/M2 above M3/M4.
    for pair in EXTRA[..4].chunks_exact(2) {
        ui.columns(2, |columns| {
            for (ui, &key) in columns.iter_mut().zip(pair) {
                let field = offset(key)..offset(key) + 3;
                let edited = draft[field.clone()] != saved[field];
                let pressed = input.button_lit(key);
                egui::Frame::new()
                    .fill(if pressed {
                        PRESSED_BG
                    } else {
                        Color32::from_rgb(35, 40, 37)
                    })
                    .stroke(egui::Stroke::new(
                        1.0_f32,
                        if edited {
                            GREEN
                        } else if pressed {
                            PRESSED_TEXT
                        } else {
                            Color32::from_rgb(56, 63, 57)
                        },
                    ))
                    .corner_radius(4)
                    .inner_margin(10)
                    .show(ui, |ui| {
                        ui.set_min_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(NAMES[key])
                                    .monospace()
                                    .size(23.)
                                    .strong()
                                    .color(if pressed {
                                        PRESSED_TEXT
                                    } else if edited {
                                        GREEN
                                    } else {
                                        TEXT
                                    }),
                            );
                            let (arrow, _) =
                                ui.allocate_exact_size(egui::vec2(14., 14.), egui::Sense::hover());
                            ui.painter().arrow(
                                arrow.left_center(),
                                egui::vec2(12., 0.),
                                egui::Stroke::new(1.0_f32, MUTED),
                            );
                            ui.add_enabled_ui(editable, |ui| binding_menu(ui, draft, key));
                        });
                        if edited {
                            ui.colored_label(MUTED, format!("В профиле: {}", binding(saved, key)));
                        }
                    });
            }
        });
    }
}

fn assignment_row(
    ui: &mut egui::Ui,
    key: usize,
    draft: &mut [u8; 840],
    saved: &[u8; 840],
    editable: bool,
    input: &Input,
) -> egui::Rect {
    let i = offset(key);
    let edited = draft[i..i + 3] != saved[i..i + 3];
    let pressed = input.button_lit(key);
    let ink = if pressed {
        PRESSED_TEXT
    } else if edited {
        GREEN
    } else {
        TEXT
    };
    let label = ["Вверх", "Вправо", "Вниз", "Влево"]
        .get(key)
        .copied()
        .unwrap_or(NAMES[key]);
    egui::Frame::new()
        .fill(if pressed {
            PRESSED_BG
        } else if key < 16 {
            Color32::from_rgb(35, 40, 37)
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(4)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.set_min_width(ui.available_width());
                let source = |ui: &mut egui::Ui| {
                    let name = if edited {
                        format!("{label}  •")
                    } else {
                        label.into()
                    };
                    ui.label(RichText::new(name).monospace().size(16.).color(ink));
                    if key >= 16 {
                        ui.colored_label(
                            MUTED,
                            if key >= 22 {
                                "Верхняя грань"
                            } else {
                                "Лицевая панель"
                            },
                        );
                    }
                };
                if key < 16 {
                    ui.add_space(6.);
                    ui.allocate_ui_with_layout(
                        egui::vec2(90., 32.),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.set_min_width(90.);
                            source(ui);
                        },
                    );
                } else {
                    source_column(ui, 34., source);
                }
                ui.add_enabled_ui(editable, |ui| binding_menu(ui, draft, key));
            })
        })
        .response
        .rect
}

fn source_column(ui: &mut egui::Ui, height: f32, contents: impl FnOnce(&mut egui::Ui)) {
    let width = ui.available_width() * 0.48;
    ui.allocate_ui_with_layout(
        egui::vec2(width, height),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_width(width);
            contents(ui);
        },
    );
}

fn binding_menu(ui: &mut egui::Ui, draft: &mut [u8; 840], key: usize) {
    let i = offset(key);
    egui::ComboBox::from_id_salt(("bind", key))
        .width(ui.available_width().min(300.))
        .selected_text(binding(draft, key))
        .show_ui(ui, |ui| {
            for target in std::iter::once(255).chain(STANDARD.iter().map(|&v| v as u8)) {
                let label = if target == 255 {
                    "По умолчанию"
                } else {
                    NAMES[target as usize]
                };
                let selected = draft[i] == target && draft[i + 2] == 0;
                if ui.selectable_label(selected, label).clicked() && !selected {
                    draft[i..i + 3].copy_from_slice(&[target, 0, 0]);
                }
            }
        });
}
fn main() -> eframe::Result {
    if std::env::args().any(|s| s == "--probe") {
        match device::load() {
            Ok(p) => {
                println!(
                    "Профиль {} · формат {}.{}",
                    p.slot + 1,
                    p.bytes[1],
                    p.bytes[0]
                );
                for &key in EXTRA.iter().chain(&STANDARD) {
                    println!("{} -> {}", NAMES[key], binding(&p.bytes, key));
                }
            }
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_icon(
                eframe::icon_data::from_png_bytes(include_bytes!("../assets/vader5.png"))
                    .expect("embedded application icon"),
            )
            .with_app_id("vader5")
            .with_inner_size([1020., 720.])
            .with_min_inner_size([560., 440.]),
        ..Default::default()
    };
    eframe::run_native(
        "Flydigi Vader 5 Pro",
        options,
        Box::new(|cc| Ok(Box::new(Editor::new(&cc.egui_ctx)))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multiple_controllers_wait_for_selection_and_keep_dirty_target() {
        let ctx = egui::Context::default();
        let mut editor = Editor::new(&ctx);
        let pad = |port: &str| device::Controller {
            label: "Vader 5 Pro".into(),
            path: std::ffi::CString::new(port).unwrap(),
        };
        let (a, b) = (pad("usb-A"), pad("usb-B"));
        let scan = |editor: &mut Editor, devices, dirty| {
            let (tx, rx) = mpsc::channel();
            tx.send(Ok(devices)).unwrap();
            editor.scan = Some(rx);
            editor.discover(&ctx, dirty);
        };
        scan(&mut editor, vec![a.clone(), b.clone()], false);
        assert!(editor.selected.is_none() && editor.work.is_none());
        editor.selected = Some(b.path.clone());
        editor.draft[123] = 42;
        scan(&mut editor, vec![a.clone()], true);
        assert_eq!(editor.selected.as_ref(), Some(&b.path));
        assert!(!editor.online() && editor.work.is_none());
        assert!(
            !editor.error,
            "disconnect must not set a profile operation error"
        );
        assert_eq!(editor.draft[123], 42);
        editor.next_retry = std::time::Instant::now() + std::time::Duration::from_secs(8);
        scan(&mut editor, vec![b.clone(), a], true);
        assert!(editor.online());
        assert_eq!(editor.selected, Some(b.path));
    }

    #[test]
    fn displays_special_bindings_without_rewriting_them() {
        let mut bytes = [0; 840];
        let i = offset(18);
        for (entry, expected) in [
            ([0xff, 0, 0], "По умолчанию"),
            ([0x20, 0, 0], "Макрос"),
            ([0xfe, 0, 0], "Клавиатура / мышь"),
            ([4, 1, 10], "A · турбо 10/с"),
        ] {
            bytes[i..i + 3].copy_from_slice(&entry);
            assert_eq!(binding(&bytes, 18), expected);
        }
    }
}
