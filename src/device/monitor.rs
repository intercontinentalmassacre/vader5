use super::{Duration, Instant, Pad, Result};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Battery {
    Discharging(u8),
    Charging(u8),
    Full,
}
impl Battery {
    fn from_status(raw: u8) -> Option<Self> {
        let level = raw & 0x0f;
        match raw >> 4 {
            2 => Some(Self::Full),
            0 if level <= 5 => Some(Self::Discharging(level * 20)),
            1 if level <= 5 => Some(Self::Charging(level * 20)),
            _ => None,
        }
    }
}

#[derive(Default, Clone, Copy)]
pub struct Input {
    button_activity: [Option<Instant>; 24],
    pub standard_press: (u64, u16),
    pub battery: Option<Battery>,
    pub buttons: u32,
    pub axes: [f32; 4],
    pub triggers: [u8; 2],
    pub gyro: [f32; 3],
}
impl Input {
    fn parse(r: &[u8]) -> Option<Self> {
        if r.len() != 32 || r[..3] != [0x5a, 0xa5, 0xef] {
            return None;
        }
        Some(Self {
            button_activity: [None; 24],
            standard_press: (0, 0),
            battery: None,
            buttons: u32::from_le_bytes(r[11..15].try_into().unwrap()),
            axes: std::array::from_fn(|i| {
                let n = 3 + 2 * i;
                (i16::from_le_bytes([r[n], r[n + 1]]) as f32 / 32767.).clamp(-1., 1.)
            }),
            triggers: [r[15], r[16]],
            gyro: std::array::from_fn(|i| {
                let n = 17 + 2 * i;
                f32::from(i16::from_le_bytes([r[n], r[n + 1]])) * 2000. / 32768.
            }),
        })
    }
    fn track_buttons(&mut self, activity: &mut [Option<Instant>; 24], now: Instant) {
        for (key, seen) in activity.iter_mut().enumerate() {
            if self.pressed(key) {
                *seen = Some(now);
            }
        }
        self.button_activity = *activity;
    }

    fn track_standard_press(&mut self, previous: &mut u16, latest: &mut (u64, u16)) {
        let mut down = self.buttons as u16;
        for key in [12, 13] {
            if self.pressed(key) {
                down |= 1 << key;
            }
        }
        let pressed = down & !*previous;
        if pressed != 0 {
            *latest = (latest.0.wrapping_add(1), pressed);
        }
        *previous = down;
        self.standard_press = *latest;
    }
    pub fn button_lit(&self, key: usize) -> bool {
        self.button_activity
            .get(key)
            .copied()
            .flatten()
            .is_some_and(|seen| seen.elapsed() < Duration::from_millis(120))
    }
    pub fn pressed(&self, k: usize) -> bool {
        self.buttons & (1 << k) != 0 || (matches!(k, 12 | 13) && self.triggers[k - 12] > 4)
    }
}
const PULSE_TIME: Duration = Duration::from_millis(500);
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Motor {
    Left,
    Right,
    LeftTrigger,
    RightTrigger,
}
impl Motor {
    fn output(self) -> [u8; 4] {
        match self {
            Self::Left => [150, 0, 0, 0],
            Self::Right => [0, 150, 0, 0],
            Self::LeftTrigger => [0, 0, 150, 0],
            Self::RightTrigger => [0, 0, 0, 150],
        }
    }
}
enum Pulse {
    Idle,
    Pending(Motor),
    Playing(Motor, Instant),
}

pub struct Monitor {
    pulse: Arc<Mutex<Pulse>>,
    state: Arc<Mutex<Result<Input>>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Monitor {
    pub fn start(profile: &super::Profile) -> Self {
        let path = profile.path.clone();
        let uid = profile.uid;
        let state = Arc::new(Mutex::new(Err("Подключение ввода…".into())));
        let stop = Arc::new(AtomicBool::new(false));
        let pulse = Arc::new(Mutex::new(Pulse::Idle));
        let worker_pulse = pulse.clone();
        let (shared, quit) = (state.clone(), stop.clone());
        let thread = thread::spawn(move || {
            if let Err(e) = stream(&path, uid, &shared, &quit, &worker_pulse) {
                *shared.lock().unwrap() = Err(e);
                *worker_pulse.lock().unwrap() = Pulse::Idle;
            }
        });
        Self {
            state,
            pulse,
            stop,
            thread: Some(thread),
        }
    }
    pub fn finished(&self) -> bool {
        self.thread.as_ref().is_none_or(|t| t.is_finished())
    }

    pub fn test(&self, motor: Motor) {
        if self.state().is_ok() {
            let mut p = self.pulse.lock().unwrap();
            if matches!(*p, Pulse::Idle) {
                *p = Pulse::Pending(motor);
            }
        }
    }
    pub fn pulse(&self) -> Option<(Motor, f32)> {
        match *self.pulse.lock().unwrap() {
            Pulse::Idle => None,
            Pulse::Pending(m) => Some((m, 0.)),
            Pulse::Playing(m, t) => Some((
                m,
                (t.elapsed().as_secs_f32() / PULSE_TIME.as_secs_f32()).min(1.),
            )),
        }
    }
    pub fn state(&self) -> Result<Input> {
        self.state.lock().unwrap().clone()
    }
}
impl Drop for Monitor {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
fn stream(
    path: &std::ffi::CStr,
    uid: [u8; 13],
    shared: &Mutex<Result<Input>>,
    stop: &AtomicBool,
    pulse: &Mutex<Pulse>,
) -> Result<()> {
    let pad = Pad::open(path)?;
    if pad.uid != uid {
        return Err("Геймпад изменился. Перечитайте профиль.".into());
    }
    let status = pad.exchange(0x10, &[])?;
    if status[6] > 1 {
        return Err("Неизвестный режим потока ввода.".into());
    }
    let enable = status[6] == 0;
    let mut motor_running = false;
    let result = (|| {
        if enable {
            pad.exchange(0x11, &[255, 1, 255, 255, 255])?;
        }
        let mut last = Instant::now();
        let mut previous_standard = 0;
        let mut latest_press = (0, 0);
        let mut button_activity = [None; 24];
        let mut battery = None;
        let mut next_heartbeat = last;
        let mut battery_updated = last;
        let mut buf = [0; 64];
        while !stop.load(Ordering::Acquire) {
            {
                let mut request = pulse.lock().unwrap();
                match *request {
                    Pulse::Pending(m) => {
                        motor_running = true;
                        pad.send(0x12, &m.output())?;
                        *request = Pulse::Playing(m, Instant::now());
                    }
                    Pulse::Playing(_, t) if t.elapsed() >= PULSE_TIME => {
                        pad.send(0x12, &[0; 4])?;
                        motor_running = false;
                        *request = Pulse::Idle;
                    }
                    _ => {}
                }
            }
            if Instant::now() >= next_heartbeat {
                pad.send(1, &[])?;
                next_heartbeat = Instant::now() + Duration::from_secs(10);
            }
            let n = pad
                .device
                .read_timeout(&mut buf, 50)
                .map_err(|e| e.to_string())?;
            if n == 32 && buf[..3] == [0x5a, 0xa5, 1] && matches!(buf[5], 130 | 144 | 145) {
                battery = Battery::from_status(buf[11]);
                battery_updated = Instant::now();
            }
            if let Some(mut input) = Input::parse(&buf[..n]) {
                input.track_standard_press(&mut previous_standard, &mut latest_press);
                input.track_buttons(&mut button_activity, Instant::now());
                if battery_updated.elapsed() < Duration::from_secs(25) {
                    input.battery = battery;
                }
                *shared.lock().unwrap() = Ok(input);
                last = Instant::now();
            } else if last.elapsed() > Duration::from_secs(2) {
                return Err(
                    "Нет данных ввода. Проверьте подключение и перечитайте профиль.".into(),
                );
            }
        }
        Ok(())
    })();
    let stopped = if motor_running {
        pad.send(0x12, &[0; 4])
    } else {
        Ok(())
    };
    if enable {
        pad.exchange(0x11, &[255, 0, 255, 255, 255])?;
    }
    result.and(stopped)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scroll_event_survives_release_and_does_not_repeat_while_held() {
        let mut down = 0;
        let mut event = (0, 0);
        let mut input = Input::default();
        for (trigger, sequence) in [(0, 0), (5, 1), (6, 1), (0, 1), (100, 2)] {
            input.triggers[0] = trigger;
            input.track_standard_press(&mut down, &mut event);
            assert_eq!(event.0, sequence);
            if sequence > 0 {
                assert_eq!(event.1, 1 << 12);
            }
        }
    }
    #[test]
    fn button_highlights_follow_physical_input() {
        let mut report = [0; 32];
        report[..3].copy_from_slice(&[0x5a, 0xa5, 0xef]);
        let mut masks: Vec<u32> = (0..24).map(|key| 1 << key).collect();
        masks.push(0xc7_4031);
        for mask in masks {
            report[11..15].copy_from_slice(&mask.to_le_bytes());
            let mut input = Input::parse(&report).unwrap();
            input.track_buttons(&mut [None; 24], Instant::now());
            for key in 0..24 {
                assert_eq!(input.button_lit(key), mask & (1 << key) != 0);
            }
        }
        report[11..15].fill(0);
        for (lt, rt) in [(5, 4), (4, 5), (255, 255)] {
            report[15..17].copy_from_slice(&[lt, rt]);
            let mut input = Input::parse(&report).unwrap();
            input.track_buttons(&mut [None; 24], Instant::now());
            assert_eq!(input.button_lit(12), lt > 4);
            assert_eq!(input.button_lit(13), rt > 4);
        }
    }
    #[test]
    fn battery_status_from_heartbeat() {
        assert_eq!(Battery::from_status(0x01), Some(Battery::Discharging(20)));
        assert_eq!(Battery::from_status(0x13), Some(Battery::Charging(60)));
        assert_eq!(Battery::from_status(0x25), Some(Battery::Full));
        assert_eq!(Battery::from_status(0xff), None);
        assert_eq!(Battery::from_status(0x06), None);
    }
    #[test]
    #[ignore = "requires a connected Vader 5 Pro"]
    fn live_device_stream_restores_flags() {
        let before = Pad::open(&super::super::single().unwrap())
            .unwrap()
            .exchange(0x10, &[])
            .unwrap();
        let monitor = Monitor::start(&super::super::load().unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if monitor.state().is_ok() {
                break;
            }
            assert!(Instant::now() < deadline, "{:?}", monitor.state().err());
            thread::sleep(Duration::from_millis(20));
        }
        drop(monitor);
        let after = Pad::open(&super::super::single().unwrap())
            .unwrap()
            .exchange(0x10, &[])
            .unwrap();
        assert_eq!(before[5..10], after[5..10]);
    }
    #[test]
    #[ignore = "requires a physical controller; pulses all four motors"]
    fn live_motor_tests_stop() {
        let before = super::super::load().unwrap();
        let m = Monitor::start(&super::super::load().unwrap());
        let deadline = Instant::now() + Duration::from_secs(5);
        while m.state().is_err() {
            assert!(Instant::now() < deadline, "{:?}", m.state().err());
            thread::sleep(Duration::from_millis(10));
        }
        for motor in [
            Motor::Left,
            Motor::Right,
            Motor::LeftTrigger,
            Motor::RightTrigger,
        ] {
            m.test(motor);
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut sent = false;
            while let Some((which, progress)) = m.pulse() {
                assert_eq!(which, motor);
                sent |= progress > 0.;
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(10));
            }
            assert!(sent);
            assert!(m.state().is_ok(), "{:?}", m.state().err());
        }
        drop(m);
        let after = super::super::load().unwrap();
        assert_eq!(before.slot, after.slot);
        assert_eq!(before.revision, after.revision);
        assert_eq!(before.lights, after.lights);
        assert_eq!(before.bytes, after.bytes);
    }
    #[test]
    fn motor_channels_are_independent() {
        for (channel, m) in [
            Motor::Left,
            Motor::Right,
            Motor::LeftTrigger,
            Motor::RightTrigger,
        ]
        .into_iter()
        .enumerate()
        {
            for (i, value) in m.output().into_iter().enumerate() {
                assert_eq!(value != 0, i == channel);
            }
        }
    }
    #[test]
    fn physical_report() {
        let mut r = [0; 32];
        r[..3].copy_from_slice(&[90, 165, 239]);
        r[13] = 4;
        r[3..5].copy_from_slice(&i16::MIN.to_le_bytes());
        r[16] = 180;
        let i = Input::parse(&r).unwrap();
        assert!(i.pressed(18) && i.pressed(13));
        assert!(!i.pressed(4));
        assert_eq!(i.axes[0], -1.);
    }
}
