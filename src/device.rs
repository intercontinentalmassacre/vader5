use hidapi::{HidApi, HidDevice};
use std::fs::{File, OpenOptions};
#[cfg(windows)]
use std::os::windows::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

const CHUNK: usize = 20;
const COUNT: usize = 42;
const SIZE: usize = COUNT * CHUNK;
type Reply = [u8; 32];
type Result<T> = std::result::Result<T, String>;

pub mod monitor;
use std::ffi::{CStr, CString};

#[derive(Clone, PartialEq, Debug)]
pub struct Controller {
    pub path: CString,
    pub label: String,
}

pub fn discover() -> Result<Vec<Controller>> {
    let api = HidApi::new().map_err(|e| e.to_string())?;
    let mut pads = Vec::new();
    for info in api
        .device_list()
        .filter(|d| d.vendor_id() == 0x37d7 && d.usage_page() == 0xffa0 && d.usage() == 1)
    {
        let path = info.path().to_owned();
        let name = info
            .product_string()
            .filter(|s| !s.is_empty())
            .unwrap_or("Flydigi");
        // Include a port identifier even when two receivers report the same serial.
        let id = path.to_bytes().iter().fold(2166136261u32, |h, b| {
            (h ^ u32::from(*b)).wrapping_mul(16777619)
        });
        pads.push(Controller {
            path,
            label: format!("{name} · {id:08X}"),
        });
    }
    pads.sort_by(|a, b| a.path.cmp(&b.path));
    pads.dedup_by(|a, b| a.path == b.path);
    Ok(pads)
}

fn single() -> Result<CString> {
    let mut devices = discover()?;
    match devices.len() {
        0 => Err("Подключите Vader 5 Pro кабелем или через USB-приёмник в режиме XInput.".into()),
        1 => Ok(devices.remove(0).path),
        _ => Err("Подключено несколько устройств. Выберите геймпад в окне приложения.".into()),
    }
}

pub struct Profile {
    pub path: CString,
    pub lights: [u8; 320],
    pub slot: u8,
    pub bytes: [u8; SIZE],
    uid: [u8; 13],
    revision: u16,
}

struct Pad {
    path: CString,
    device: HidDevice,
    _lock: File,
    uid: [u8; 13],
}

fn checksum(data: &[u8]) -> u8 {
    data.iter().copied().fold(0, u8::wrapping_add)
}

fn report(command: u8, payload: &[u8]) -> [u8; 33] {
    // HIDAPI on Windows requires a zero report ID for this unnumbered collection.
    let mut r = [0; 33];
    r[1..5].copy_from_slice(&[0x5a, 0xa5, command, 2 + payload.len() as u8]);
    let end = 5 + payload.len();
    r[5..end].copy_from_slice(payload);
    r[end] = checksum(&r[3..end]);
    r
}

impl Pad {
    fn open(path: &CStr) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).write(true);
        #[cfg(windows)]
        let lock_path = {
            options.share_mode(0);
            std::env::temp_dir().join("vader5-config.lock")
        };
        #[cfg(target_os = "linux")]
        let lock_path = {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
            let dir = std::env::var_os("XDG_RUNTIME_DIR")
                .map(std::path::PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|p| std::path::PathBuf::from(p).join(".cache"))
                })
                .ok_or("Не удалось найти каталог сеанса пользователя.")?;
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            dir.join("vader5-config.lock")
        };
        let lock = options
            .open(lock_path)
            .map_err(|e| format!("Не удалось начать сеанс настройки. {e}"))?;
        #[cfg(target_os = "linux")]
        lock.try_lock().map_err(|e| {
            format!("Не удалось занять геймпад. Закройте другое окно настройки. {e}")
        })?;
        let api = HidApi::new().map_err(|e| e.to_string())?;
        let device = api.open_path(path).map_err(|e| {
            #[cfg(windows)]
            let hint = "Закройте Space Station и повторите.";
            #[cfg(target_os = "linux")]
            let hint = "Проверьте правило udev для Vader 5 Pro и переподключите USB.";
            format!("Не удалось открыть геймпад. {hint} {e}")
        })?;
        let mut pad = Self {
            path: path.to_owned(),
            device,
            _lock: lock,
            uid: [0; 13],
        };
        if !matches!(pad.exchange(1, &[])?[5], 130 | 144 | 145) {
            return Err("Устройство не подтвердило модель Vader 5 Pro.".into());
        }
        pad.uid = pad.exchange(4, &[])?[5..18].try_into().unwrap();
        Ok(pad)
    }

    fn send(&self, cmd: u8, payload: &[u8]) -> Result<()> {
        let r = report(cmd, payload);
        let n = self
            .device
            .write(&r)
            .map_err(|e| format!("Ошибка передачи USB. {e}"))?;
        if n != r.len() {
            return Err("Геймпад принял неполный пакет.".into());
        }
        Ok(())
    }

    fn receive(&self, cmd: u8, index: Option<u8>, deadline: Instant) -> Result<Reply> {
        loop {
            let left = deadline
                .saturating_duration_since(Instant::now())
                .as_millis() as i32;
            if left == 0 {
                break;
            }
            let mut buf = [0; 64];
            let n = self
                .device
                .read_timeout(&mut buf, left)
                .map_err(|e| format!("Ошибка чтения USB. {e}"))?;
            if n == 0 {
                break;
            }
            // Heartbeat uses byte 31 for device information on firmware 7.1.5.0.
            if n == 32
                && buf[..3] == [0x5a, 0xa5, cmd]
                && index.is_none_or(|i| buf[5] == i)
                && (cmd == 1 || checksum(&buf[2..31]) == buf[31])
            {
                return Ok(buf[..32].try_into().unwrap());
            }
        }
        Err(format!(
            "Геймпад не ответил на команду {cmd:02X}. Проверьте подключение и повторите чтение."
        ))
    }

    fn exchange(&self, cmd: u8, payload: &[u8]) -> Result<Reply> {
        // Reads and individually indexed upload packets can be sent again after a lost ACK.
        let once = || {
            self.send(cmd, payload)?;
            self.receive(
                cmd,
                if matches!(cmd, 0xa5 | 0xa9) {
                    Some(payload[0])
                } else {
                    None
                },
                Instant::now()
                    + Duration::from_secs(if matches!(cmd, 0xa6 | 0xaa) { 10 } else { 2 }),
            )
        };
        match once() {
            Err(_) if matches!(cmd, 1 | 4 | 0x10 | 0xa1 | 0xa4 | 0xa5 | 0xa8 | 0xa9) => once(),
            result => result,
        }
    }

    fn active(&self) -> Result<(u8, u16)> {
        let r = self.exchange(0xa1, &[])?;
        let slot = r[5];
        if slot >= 4 {
            return Err("Для редактирования переключите геймпад в режим XInput.".into());
        }
        let offset = 6 + usize::from(slot) * 2;
        Ok((slot, u16::from_le_bytes([r[offset], r[offset + 1]])))
    }

    fn blob<const N: usize>(&self, cmd: u8, slot: u8) -> Result<[u8; N]> {
        match self.read_blob_once(cmd, slot) {
            Ok(data) => Ok(data),
            Err(error) => {
                if self.active()?.0 != slot {
                    return Err(error);
                }
                self.read_blob_once(cmd, slot)
            }
        }
    }
    fn read_blob_once<const N: usize>(&self, cmd: u8, slot: u8) -> Result<[u8; N]> {
        self.send(cmd, &[slot, CHUNK as u8])?;
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut bytes = [0; N];
        let mut received = vec![false; N / CHUNK];
        while received.contains(&false) {
            let r = self.receive(cmd, None, deadline)?;
            let i = usize::from(r[4]);
            if usize::from(r[3]) != received.len() || i >= received.len() || r[5] != slot {
                return Err("Геймпад вернул неожиданный формат профиля.".into());
            }
            bytes[i * CHUNK..(i + 1) * CHUNK].copy_from_slice(&r[6..6 + CHUNK]);
            received[i] = true;
        }
        Ok(bytes)
    }
    fn profile(&self) -> Result<Profile> {
        let (slot, revision) = self.active()?;
        let bytes = self.blob(0xa3, slot)?;
        if bytes[..2] != [2, 3] {
            return Err(format!(
                "Формат профиля {}.{} пока не поддерживается.",
                bytes[1], bytes[0]
            ));
        }
        let lights = self.blob(0xa7, slot)?;
        if self.active()? != (slot, revision) {
            return Err("Активный профиль изменился во время чтения. Повторите чтение.".into());
        }
        Ok(Profile {
            path: self.path.clone(),
            lights,
            slot,
            bytes,
            uid: self.uid,
            revision,
        })
    }
}

pub fn load() -> Result<Profile> {
    load_at(&single()?)
}

pub fn load_at(path: &CStr) -> Result<Profile> {
    Pad::open(path)?.profile()
}

fn can_resume<const N: usize>(
    old: &[u8; N],
    current: &[u8; N],
    target: &[u8; N],
    retry: bool,
) -> bool {
    old.chunks_exact(CHUNK)
        .zip(current.chunks_exact(CHUNK))
        .zip(target.chunks_exact(CHUNK))
        .all(|((old, now), want)| now == old || (retry && now == want))
}

pub fn save(
    original: &Profile,
    draft: &[u8; SIZE],
    lights: &[u8; 320],
    retry: bool,
) -> Result<Profile> {
    let pad = Pad::open(&original.path)?;
    let current = pad.profile()?;
    if current.uid != original.uid || current.slot != original.slot {
        return Err("Геймпад или профиль изменился. Считайте назначения заново.".into());
    }
    let mapping_changed = original.bytes != *draft;
    let lighting_changed = original.lights != *lights;
    if !mapping_changed && !lighting_changed {
        return Ok(current);
    }
    if original
        .bytes
        .iter()
        .zip(draft)
        .enumerate()
        .any(|(i, (a, b))| a != b && !crate::tuning::editable_byte(i))
    {
        return Err("Изменения затрагивают неизвестные поля профиля.".into());
    }
    let mut next = *draft;
    let tag = if mapping_changed {
        (original.revision.wrapping_add(1) % u16::MAX).to_le_bytes()
    } else {
        [original.bytes[225], original.bytes[226]]
    };
    next[225..227].copy_from_slice(&tag);
    // A previous attempt can finish the commit and lose the activation reply.
    let committed = retry
        && current.revision.to_le_bytes() == tag
        && current.bytes == next
        && current.lights == *lights;
    if (!committed && current.revision != original.revision)
        || !can_resume(&original.bytes, &current.bytes, &next, retry)
        || !can_resume(&original.lights, &current.lights, lights, retry)
    {
        return Err(
            "Профиль изменился на устройстве. Отмените черновик и прочитайте профиль заново."
                .into(),
        );
    }
    if current.bytes != next {
        upload(0xa4, current.slot, &current.bytes, &next, |cmd, p| {
            pad.exchange(cmd, p).map(|_| ())
        })?;
    }
    if lighting_changed && !committed {
        upload(0xa8, current.slot, &current.lights, lights, |cmd, p| {
            pad.exchange(cmd, p).map(|_| ())
        })?;
    }
    if pad.active()? != (current.slot, current.revision) {
        return Err("Активный профиль изменился во время записи. Считайте профиль заново.".into());
    }
    if lighting_changed {
        // AA commits the running configuration and signals the LED coprocessor.
        // The profile version alone cannot confirm that the signal was delivered.
        let ack = pad.exchange(0xaa, &tag)?;
        if ack[4] != 0 {
            return Err(format!(
                "Геймпад отклонил применение подсветки ({}).",
                ack[4]
            ));
        }
    } else if !committed
        && let Err(e) = pad.exchange(0xa6, &tag)
        && pad.active()? != (current.slot, u16::from_le_bytes(tag))
    {
        return Err(e);
    }
    std::thread::sleep(Duration::from_millis(200));
    let saved = pad.profile()?;
    if saved.slot != current.slot
        || saved.bytes != next
        || saved.lights != *lights
        || saved.revision.to_le_bytes() != tag
    {
        return Err(
            "Геймпад не подтвердил изменения при повторном чтении. Обновите профиль.".into(),
        );
    }
    Ok(saved)
}

fn upload(
    start: u8,
    slot: u8,
    before: &[u8],
    bytes: &[u8],
    mut exchange: impl FnMut(u8, &[u8]) -> Result<()>,
) -> Result<()> {
    assert_eq!(before.len(), bytes.len());
    assert!(bytes.len().is_multiple_of(CHUNK));
    let packets = bytes.len() / CHUNK;
    // Lighting is one playback table. Send its header and all frames in one transaction.
    let changed = |n: usize| {
        start == 0xa8 || before[n * CHUNK..(n + 1) * CHUNK] != bytes[n * CHUNK..(n + 1) * CHUNK]
    };
    let mut cursor = 0;
    while cursor < packets {
        if !changed(cursor) {
            cursor += 1;
            continue;
        }
        let first = cursor;
        while cursor < packets && changed(cursor) {
            cursor += 1;
        }
        exchange(
            start,
            &[slot, first as u8, (cursor - first) as u8, CHUNK as u8],
        )?;
        for (index, chunk) in bytes[first * CHUNK..cursor * CHUNK]
            .chunks_exact(CHUNK)
            .enumerate()
        {
            let mut p = [0; 1 + CHUNK];
            p[0] = index as u8;
            p[1..].copy_from_slice(chunk);
            exchange(start + 1, &p)
                .map_err(|e| format!("{e} Пакет {} из {packets}.", first + index + 1))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_profile_frame_matches_wire() {
        let frame = report(0xa3, &[1, 20]);
        assert_eq!(frame[..8], [0, 0x5a, 0xa5, 0xa3, 4, 1, 20, 188]);
        assert_eq!(frame[8..], [0; 25]);
    }

    #[test]
    fn upload_round_trips_all_bytes_and_stops_on_failed_ack() {
        let bytes: [u8; SIZE] = std::array::from_fn(|i| (i % 251) as u8);
        let mut reconstructed = Vec::new();
        upload(0xa4, 2, &[255; SIZE], &bytes, |cmd, payload| {
            match cmd {
                0xa4 => assert_eq!(payload, [2, 0, 42, 20]),
                0xa5 => {
                    assert_eq!(usize::from(payload[0]), reconstructed.len() / 20);
                    reconstructed.extend_from_slice(&payload[1..]);
                }
                _ => panic!("unexpected command"),
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(reconstructed, bytes);
        let mut calls = 0;
        let result = upload(0xa4, 2, &[255; SIZE], &bytes, |_, _| {
            calls += 1;
            if calls == 5 {
                Err("lost ack".into())
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        assert_eq!(calls, 5);
    }

    #[test]
    fn retry_accepts_only_original_or_requested_chunks() {
        let old = [0; 60];
        let mut next = old;
        next[3] = 9;
        next[44] = 2;
        let mut staged = old;
        staged[3] = 9;
        assert!(can_resume(&old, &staged, &next, true));
        assert!(!can_resume(&old, &staged, &next, false));
        staged[30] = 7;
        assert!(!can_resume(&old, &staged, &next, true));
    }
    #[test]
    fn sparse_batches_use_absolute_start_and_local_indices() {
        let old = [0; SIZE];
        let mut next = old;
        for n in [2, 3, 11] {
            next[n * CHUNK] = 88;
        }
        let mut commands = Vec::new();
        upload(0xa4, 1, &old, &next, |cmd, data| {
            commands.push((cmd, data.to_vec()));
            Ok(())
        })
        .unwrap();
        assert_eq!(commands[0], (0xa4, vec![1, 2, 2, 20]));
        assert_eq!(commands[3], (0xa4, vec![1, 11, 1, 20]));
        assert_eq!(
            commands
                .iter()
                .filter(|x| x.0 == 0xa5)
                .map(|x| x.1[0])
                .collect::<Vec<_>>(),
            [0, 1, 0]
        );
    }
    #[test]
    #[ignore = "requires a connected controller; stages identical settings without committing"]
    fn hardware_combined_staging() -> Result<()> {
        let pad = Pad::open(&single()?).unwrap();
        let before = pad.profile().unwrap();
        for (cmd, bytes, chunks) in [
            (0xa4, &before.bytes[..], &[2, 3, 11][..]),
            (0xa8, &before.lights[..], &[0, 1, 2, 3][..]),
        ] {
            let mut comparison = bytes.to_vec();
            for &i in chunks {
                comparison[i * CHUNK] ^= 1;
            }
            upload(cmd, before.slot, &comparison, bytes, |cmd, p| {
                pad.exchange(cmd, p).map(|_| ())
            })?;
        }
        let after = pad.profile()?;
        assert_eq!((after.slot, after.revision), (before.slot, before.revision));
        assert_eq!(after.bytes, before.bytes);
        assert_eq!(after.lights, before.lights);
        Ok(())
    }
}
