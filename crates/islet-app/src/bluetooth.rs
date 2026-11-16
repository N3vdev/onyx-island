//! Bluetooth earbuds, headphones and speakers: when they connect and leave,
//! and how charged they are.
//!
//! Windows' device watcher says when a paired device connects or
//! disconnects, as it happens: nothing polls for that. The battery level is
//! a property Windows keeps on the device (from the hands-free link, or
//! Bluetooth LE's battery service), read when the device connects and then
//! once a minute while one is connected. With none connected, the thread
//! sleeps until the watcher wakes it.
//!
//! Device names stay out of the log.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, RecvTimeoutError, Sender};
use islet_core::island::{DeviceBadge, DeviceKind};
use windows::core::{GUID, HSTRING};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind, DeviceInformationUpdate, DeviceWatcher};
use windows_collections::{IIterable, IMapView};
use windows::Foundation::{IPropertyValue, TypedEventHandler};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDevicePropertyW,
    DIGCF_ALLCLASSES, DIGCF_PRESENT, SP_DEVINFO_DATA,
};
use windows::Win32::Devices::Properties::DEVPROPTYPE;
use windows::Win32::Foundation::{DEVPROPKEY, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

/// Posted to the island when there is news (see [`Bluetooth::take`]).
pub const WM_APP_BLUETOOTH: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 34;

/// Classic Bluetooth, for the watcher's filter.
const CLASSIC: &str = "{e0cbf06c-cd8b-4647-bb8a-263b43f0f974}";
const CONNECTED: &str = "System.Devices.Aep.IsConnected";
const ADDRESS: &str = "System.Devices.Aep.DeviceAddress";
const MAJOR: &str = "System.Devices.Aep.Bluetooth.Cod.Major";
const MINOR: &str = "System.Devices.Aep.Bluetooth.Cod.Minor";

/// The battery level Windows shows in Settings: a byte, in percent.
const BATTERY: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5),
    pid: 2,
};
/// DEVPKEY_Bluetooth_DeviceAddress: twelve hex digits.
const DEVICE_ADDRESS: DEVPROPKEY = DEVPROPKEY {
    fmtid: GUID::from_u128(0x2bd67d8b_8beb_48d5_87e0_6cda3428040a),
    pid: 1,
};

/// A just-connected device is announced once its battery is known, or
/// after this long without it.
const ANNOUNCE_WAIT: Duration = Duration::from_millis(3_000);
/// A device that leaves within this long of connecting was a blip: no
/// notice either way.
const BLIP: Duration = Duration::from_millis(1_500);
/// While a device is connected its battery is read this often.
const BATTERY_EVERY: Duration = Duration::from_secs(60);
/// Low-battery notices, once each per charge.
const LOW: [u8; 2] = [20, 10];

/// Something to tell the user.
#[derive(Clone, Debug, PartialEq)]
pub enum News {
    Connected { name: String, badge: DeviceBadge },
    Disconnected { name: String, badge: DeviceBadge },
    Low { name: String, badge: DeviceBadge },
}

#[derive(Default)]
struct Shared {
    news: Vec<News>,
    /// The connected audio device, for the music card.
    current: Option<DeviceBadge>,
}

pub struct Bluetooth {
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Msg>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Bluetooth {
    pub fn start(island: HWND) -> Self {
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (tx, rx) = unbounded::<Msg>();
        let thread_shared = shared.clone();
        let thread_tx = tx.clone();
        let island = island.0 as isize;
        let thread = std::thread::Builder::new()
            .name("bluetooth".to_owned())
            .spawn(move || {
                // SAFETY: this thread's own COM apartment, for the watcher.
                let com = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
                let watcher = match watch(thread_tx) {
                    Ok(watcher) => Some(watcher),
                    Err(error) => {
                        tracing::warn!(%error, "Bluetooth watcher unavailable");
                        None
                    }
                };
                let mut state = State::default();
                let post = || {
                    // SAFETY: a non-blocking post to the island window.
                    unsafe {
                        let _ = PostMessageW(Some(HWND(island as *mut _)), WM_APP_BLUETOOTH, WPARAM(0), LPARAM(0));
                    }
                };
                loop {
                    let message = match state.wait() {
                        Some(wait) => rx.recv_timeout(wait),
                        None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
                    };
                    let news = match message {
                        Ok(Msg::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                        Ok(message) => state.handle(message, Instant::now(), &read_batteries),
                        Err(RecvTimeoutError::Timeout) => state.tick(Instant::now(), &read_batteries),
                    };
                    let current = state.current();
                    if let Ok(mut shared) = thread_shared.lock() {
                        let changed = !news.is_empty() || shared.current != current;
                        shared.news.extend(news);
                        shared.current = current;
                        if changed {
                            post();
                        }
                    }
                }
                if let Some(watcher) = watcher {
                    let _ = watcher.Stop();
                }
                if com {
                    // SAFETY: balances the CoInitializeEx above.
                    unsafe { CoUninitialize() };
                }
            })
            .ok();
        Self { shared, tx, thread }
    }

    /// What happened since last asked, and the device connected now.
    pub fn take(&self) -> (Vec<News>, Option<DeviceBadge>) {
        self.shared
            .lock()
            .map(|mut shared| (std::mem::take(&mut shared.news), shared.current))
            .unwrap_or_default()
    }
}

impl Drop for Bluetooth {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// From the watcher's callbacks to the thread.
enum Msg {
    Seen { id: String, name: String, facts: Facts },
    Changed { id: String, facts: Facts },
    Gone { id: String },
    /// The watcher has reported every paired device.
    Ready,
    Shutdown,
}

/// What a watcher report says (each may be missing from an update).
#[derive(Clone, Debug, Default, PartialEq)]
struct Facts {
    connected: Option<bool>,
    /// Twelve upper-case hex digits.
    address: Option<String>,
    major: Option<u32>,
    minor: Option<u32>,
}

fn watch(tx: Sender<Msg>) -> windows::core::Result<DeviceWatcher> {
    let filter = HSTRING::from(format!(
        "System.Devices.Aep.ProtocolId:=\"{CLASSIC}\" AND System.Devices.Aep.IsPaired:=System.StructuredQueryType.Boolean#True"
    ));
    let properties: IIterable<HSTRING> = vec![
        HSTRING::from(CONNECTED),
        HSTRING::from(ADDRESS),
        HSTRING::from(MAJOR),
        HSTRING::from(MINOR),
    ]
    .into();
    let watcher = DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(
        &filter,
        &properties,
        DeviceInformationKind::AssociationEndpoint,
    )?;
    let added = tx.clone();
    watcher.Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(move |_, info| {
        if let Ok(info) = info.ok() {
            let _ = added.send(Msg::Seen {
                id: info.Id()?.to_string(),
                name: info.Name()?.to_string(),
                facts: facts(&info.Properties()?),
            });
        }
        Ok(())
    }))?;
    let updated = tx.clone();
    watcher.Updated(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, update| {
        if let Ok(update) = update.ok() {
            let _ = updated.send(Msg::Changed { id: update.Id()?.to_string(), facts: facts(&update.Properties()?) });
        }
        Ok(())
    }))?;
    let removed = tx.clone();
    watcher.Removed(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, update| {
        if let Ok(update) = update.ok() {
            let _ = removed.send(Msg::Gone { id: update.Id()?.to_string() });
        }
        Ok(())
    }))?;
    let ready = tx;
    watcher.EnumerationCompleted(&TypedEventHandler::<DeviceWatcher, windows::core::IInspectable>::new(move |_, _| {
        let _ = ready.send(Msg::Ready);
        Ok(())
    }))?;
    watcher.Start()?;
    Ok(watcher)
}

fn facts(properties: &IMapView<HSTRING, windows::core::IInspectable>) -> Facts {
    let value = |key: &str| -> Option<IPropertyValue> {
        let key = HSTRING::from(key);
        if !properties.HasKey(&key).unwrap_or(false) {
            return None;
        }
        properties.Lookup(&key).ok().and_then(|value| windows::core::Interface::cast::<IPropertyValue>(&value).ok())
    };
    let number = |key: &str| {
        let value = value(key)?;
        value
            .GetUInt32()
            .ok()
            .or_else(|| value.GetUInt16().ok().map(u32::from))
            .or_else(|| value.GetUInt8().ok().map(u32::from))
    };
    Facts {
        connected: value(CONNECTED).and_then(|value| value.GetBoolean().ok()),
        address: value(ADDRESS).and_then(|value| value.GetString().ok()).map(|text| normalize_address(&text.to_string())),
        major: number(MAJOR),
        minor: number(MINOR),
    }
}

/// "08:12:87:e1:2e:64" and "081287E12E64" alike as "081287E12E64".
fn normalize_address(text: &str) -> String {
    text.chars().filter(char::is_ascii_hexdigit).collect::<String>().to_uppercase()
}

/// What kind of audio device this is, or `None` for one that is not
/// (a mouse, a phone, a controller).
fn audio_kind(name: &str, major: Option<u32>, minor: Option<u32>) -> Option<DeviceKind> {
    let lower = name.to_lowercase();
    let named = |words: &[&str]| words.iter().any(|word| lower.contains(word));
    let audio = match major {
        // Audio/Video.
        Some(4) => true,
        Some(_) => false,
        None => named(&["bud", "pods", "headphone", "headset", "earphone", "speaker", "wh-", "wf-"]),
    };
    if !audio {
        return None;
    }
    if named(&["bud", "pods", "earphone", "wf-", "ear ("]) {
        return Some(DeviceKind::Earbuds);
    }
    // Minor classes: 5 loudspeaker, 6 headphones, 1 headset, 2 hands-free.
    Some(match minor {
        Some(5) => DeviceKind::Speaker,
        _ if named(&["speaker", "soundlink", "flip", "charge", "boom", "soundcore motion"]) => DeviceKind::Speaker,
        Some(1 | 2) => DeviceKind::Earbuds,
        _ => DeviceKind::Headphones,
    })
}

/// The battery of each Bluetooth device that reports one, by address.
fn read_batteries() -> HashMap<String, u8> {
    let mut found = HashMap::new();
    for enumerator in [windows::core::w!("BTHENUM"), windows::core::w!("BTHLE")] {
        // SAFETY: the device list is created, walked and destroyed here.
        unsafe {
            let Ok(list) = SetupDiGetClassDevsW(None, enumerator, None, DIGCF_PRESENT | DIGCF_ALLCLASSES) else {
                continue;
            };
            let mut index = 0;
            loop {
                let mut info = SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
                if SetupDiEnumDeviceInfo(list, index, &mut info).is_err() {
                    break;
                }
                index += 1;
                let mut kind = DEVPROPTYPE::default();
                let mut level = [0u8; 1];
                if SetupDiGetDevicePropertyW(list, &info, &BATTERY, &mut kind, Some(&mut level), None, 0).is_err() {
                    continue;
                }
                let mut address = [0u8; 64];
                let mut size = 0u32;
                if SetupDiGetDevicePropertyW(list, &info, &DEVICE_ADDRESS, &mut kind, Some(&mut address), Some(&mut size), 0).is_err() {
                    continue;
                }
                let wide: Vec<u16> = address[..size.min(64) as usize]
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .take_while(|&unit| unit != 0)
                    .collect();
                let address = normalize_address(&String::from_utf16_lossy(&wide));
                if !address.is_empty() && level[0] <= 100 {
                    found.entry(address).or_insert(level[0]);
                }
            }
            let _ = SetupDiDestroyDeviceInfoList(list);
        }
    }
    found
}

/// One paired device.
#[derive(Clone, Debug)]
struct Device {
    name: String,
    address: Option<String>,
    kind: Option<DeviceKind>,
    connected: bool,
    battery: Option<u8>,
    /// When it connected, while its notice waits for the battery.
    pending: Option<Instant>,
    /// Its "Connected" notice was shown (so leaving gets one too).
    announced: bool,
    connected_at: Option<Instant>,
    /// The lowest low-battery notice shown this charge.
    warned: Option<u8>,
}

impl Device {
    fn badge(&self) -> Option<DeviceBadge> {
        Some(DeviceBadge { kind: self.kind?, battery: self.battery })
    }
}

/// The thread's view of the paired devices. Pure apart from the battery
/// reader it is handed, so it can be tested.
#[derive(Default)]
struct State {
    devices: HashMap<String, Device>,
    /// The watcher has listed what was already there: only changes after
    /// this are news (Onyx starting with earbuds in is not).
    ready: bool,
    last_battery_read: Option<Instant>,
}

impl State {
    fn handle(&mut self, message: Msg, now: Instant, batteries: &dyn Fn() -> HashMap<String, u8>) -> Vec<News> {
        let mut news = Vec::new();
        match message {
            Msg::Seen { id, name, facts } => {
                let connected = facts.connected.unwrap_or(false);
                let device = Device {
                    kind: audio_kind(&name, facts.major, facts.minor),
                    name,
                    address: facts.address,
                    connected: false,
                    battery: None,
                    pending: None,
                    announced: false,
                    connected_at: None,
                    warned: None,
                };
                self.devices.insert(id.clone(), device);
                if connected {
                    news.extend(self.set_connected(&id, true, now, batteries));
                }
            }
            Msg::Changed { id, facts } => {
                if let Some(device) = self.devices.get_mut(&id) {
                    if let Some(address) = facts.address {
                        device.address = Some(address);
                    }
                    if facts.major.is_some() || facts.minor.is_some() {
                        device.kind = audio_kind(&device.name, facts.major, facts.minor).or(device.kind);
                    }
                }
                if let Some(connected) = facts.connected {
                    news.extend(self.set_connected(&id, connected, now, batteries));
                }
            }
            Msg::Gone { id } => {
                news.extend(self.set_connected(&id, false, now, batteries));
                self.devices.remove(&id);
            }
            Msg::Ready => {
                self.ready = true;
                self.read_batteries(now, batteries);
            }
            Msg::Shutdown => {}
        }
        news.extend(self.tick(now, batteries));
        news
    }

    fn set_connected(&mut self, id: &str, connected: bool, now: Instant, batteries: &dyn Fn() -> HashMap<String, u8>) -> Vec<News> {
        let ready = self.ready;
        let Some(device) = self.devices.get_mut(id) else { return Vec::new() };
        if device.connected == connected || device.kind.is_none() {
            device.connected = connected;
            return Vec::new();
        }
        device.connected = connected;
        if connected {
            // Connected before Onyx started: no time (anything connected
            // since counts as newer).
            device.connected_at = ready.then_some(now);
            device.warned = None;
            device.pending = ready.then_some(now);
            self.read_batteries(now, batteries);
            return Vec::new();
        }
        let blip = device.connected_at.is_some_and(|at| now.duration_since(at) < BLIP);
        // (A device in before Onyx started has no time, and is no blip.)
        device.pending = None;
        device.connected_at = None;
        let announced = std::mem::take(&mut device.announced);
        match device.badge() {
            Some(badge) if ready && announced && !blip => {
                vec![News::Disconnected { name: device.name.clone(), badge: DeviceBadge { battery: None, ..badge } }]
            }
            _ => Vec::new(),
        }
    }

    fn read_batteries(&mut self, now: Instant, batteries: &dyn Fn() -> HashMap<String, u8>) {
        let levels = batteries();
        self.last_battery_read = Some(now);
        for device in self.devices.values_mut().filter(|device| device.connected) {
            if let Some(level) = device.address.as_ref().and_then(|address| levels.get(address)) {
                device.battery = Some(*level);
            }
        }
    }

    /// Announcements due, and batteries due a read.
    fn tick(&mut self, now: Instant, batteries: &dyn Fn() -> HashMap<String, u8>) -> Vec<News> {
        let any_connected = self.devices.values().any(|device| device.connected && device.kind.is_some());
        let waiting = self.devices.values().any(|device| device.pending.is_some());
        let stale = self.last_battery_read.is_none_or(|at| now.duration_since(at) >= BATTERY_EVERY);
        if any_connected && (stale || (waiting && self.last_battery_read.is_none_or(|at| now.duration_since(at) >= Duration::from_millis(500)))) {
            self.read_batteries(now, batteries);
        }
        let mut news = Vec::new();
        for device in self.devices.values_mut() {
            let Some(badge) = device.badge() else { continue };
            if let Some(since) = device.pending {
                if badge.battery.is_some() || now.duration_since(since) >= ANNOUNCE_WAIT {
                    device.pending = None;
                    device.announced = true;
                    news.push(News::Connected { name: device.name.clone(), badge });
                }
                continue;
            }
            if !device.connected {
                continue;
            }
            match badge.battery {
                // Charged again: warn again next time it runs down.
                Some(level) if level > 30 => device.warned = None,
                Some(level) => {
                    let due = LOW.iter().copied().filter(|&mark| level <= mark).min();
                    if let Some(mark) = due.filter(|&mark| device.warned.is_none_or(|warned| mark < warned)) {
                        device.warned = Some(mark);
                        news.push(News::Low { name: device.name.clone(), badge });
                    }
                }
                None => {}
            }
        }
        news
    }

    /// How long until something is due, or `None` to wait for the watcher.
    fn wait(&self) -> Option<Duration> {
        if self.devices.values().any(|device| device.pending.is_some()) {
            return Some(Duration::from_millis(500));
        }
        self.devices.values().any(|device| device.connected && device.kind.is_some()).then_some(BATTERY_EVERY)
    }

    /// The audio device connected now: the most recently connected one,
    /// and of those already in at startup, one that reports its battery.
    fn current(&self) -> Option<DeviceBadge> {
        self.devices
            .values()
            .filter(|device| device.connected && device.kind.is_some())
            .max_by_key(|device| (device.connected_at, device.battery.is_some()))
            .and_then(Device::badge)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUDS: &str = "OnePlus Nord Buds 3 Pro";

    fn seen(id: &str, name: &str, connected: bool) -> Msg {
        Msg::Seen {
            id: id.into(),
            name: name.into(),
            facts: Facts { connected: Some(connected), address: Some("081287E12E64".into()), major: Some(4), minor: Some(1) },
        }
    }

    fn connect(id: &str, connected: bool) -> Msg {
        Msg::Changed { id: id.into(), facts: Facts { connected: Some(connected), ..Facts::default() } }
    }

    fn battery(level: u8) -> impl Fn() -> HashMap<String, u8> {
        move || HashMap::from([("081287E12E64".to_owned(), level)])
    }

    #[test]
    fn earbuds_already_in_at_startup_are_shown_but_not_announced() {
        let mut state = State::default();
        let t = Instant::now();
        assert!(state.handle(seen("a", BUDS, true), t, &battery(90)).is_empty());
        assert!(state.handle(Msg::Ready, t, &battery(90)).is_empty());
        assert_eq!(state.current(), Some(DeviceBadge { kind: DeviceKind::Earbuds, battery: Some(90) }));
    }

    #[test]
    fn connecting_is_announced_with_the_battery_and_leaving_is_too() {
        let mut state = State::default();
        let t = Instant::now();
        state.handle(seen("a", BUDS, false), t, &battery(80));
        state.handle(Msg::Ready, t, &battery(80));
        let news = state.handle(connect("a", true), t, &battery(80));
        let badge = DeviceBadge { kind: DeviceKind::Earbuds, battery: Some(80) };
        assert_eq!(news, vec![News::Connected { name: BUDS.into(), badge }]);
        let later = t + Duration::from_secs(30);
        let news = state.handle(connect("a", false), later, &battery(80));
        assert_eq!(news, vec![News::Disconnected { name: BUDS.into(), badge: DeviceBadge { battery: None, ..badge } }]);
        assert_eq!(state.current(), None);
    }

    #[test]
    fn without_a_battery_reading_the_notice_waits_a_moment_then_goes_anyway() {
        let mut state = State::default();
        let t = Instant::now();
        let none = || HashMap::new();
        state.handle(seen("a", BUDS, false), t, &none);
        state.handle(Msg::Ready, t, &none);
        assert!(state.handle(connect("a", true), t, &none).is_empty());
        assert!(state.tick(t + Duration::from_millis(1_000), &none).is_empty());
        let news = state.tick(t + ANNOUNCE_WAIT, &none);
        assert!(matches!(&news[..], [News::Connected { badge: DeviceBadge { battery: None, .. }, .. }]));
    }

    #[test]
    fn a_blip_says_nothing_and_a_mouse_is_not_audio() {
        let mut state = State::default();
        let t = Instant::now();
        state.handle(Msg::Ready, t, &battery(50));
        state.handle(seen("a", BUDS, false), t, &battery(50));
        // Connects and drops before anything was said.
        state.handle(connect("a", true), t, &|| HashMap::new());
        assert!(state.handle(connect("a", false), t + Duration::from_millis(400), &battery(50)).is_empty());
        let mouse = Msg::Seen {
            id: "m".into(),
            name: "MX Master 3".into(),
            facts: Facts { connected: Some(false), address: Some("AA".into()), major: Some(5), minor: Some(2) },
        };
        state.handle(mouse, t, &battery(50));
        assert!(state.handle(connect("m", true), t, &battery(50)).is_empty());
        assert_eq!(state.current(), None);
    }

    #[test]
    fn running_low_warns_at_twenty_then_ten_once_each_per_charge() {
        let mut state = State::default();
        let t = Instant::now();
        state.handle(seen("a", BUDS, true), t, &battery(25));
        state.handle(Msg::Ready, t, &battery(25));
        let at = |minutes: u64| t + Duration::from_secs(minutes * 60);
        assert!(state.tick(at(1), &battery(21)).is_empty());
        assert!(matches!(&state.tick(at(2), &battery(19))[..], [News::Low { .. }]));
        assert!(state.tick(at(3), &battery(15)).is_empty(), "once at twenty");
        assert!(matches!(&state.tick(at(4), &battery(9))[..], [News::Low { .. }]));
        assert!(state.tick(at(5), &battery(8)).is_empty(), "once at ten");
        state.tick(at(6), &battery(80));
        assert!(matches!(&state.tick(at(7), &battery(18))[..], [News::Low { .. }]), "charged, then low again");
    }

    /// Reads this PC's paired devices and batteries through Windows.
    #[test]
    #[ignore = "reads this PC's Bluetooth devices"]
    fn this_pcs_devices() {
        // SAFETY: the test thread's own apartment.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        println!("batteries by address: {:?}", read_batteries());
        let (tx, rx) = unbounded();
        let watcher = watch(tx).expect("watcher");
        let mut state = State::default();
        let deadline = Instant::now() + Duration::from_secs(8);
        while let Ok(message) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            let ready = matches!(message, Msg::Ready);
            state.handle(message, Instant::now(), &read_batteries);
            if ready {
                break;
            }
        }
        let _ = watcher.Stop();
        for device in state.devices.values() {
            println!("{:?} connected={} battery={:?} address={:?}", device.kind, device.connected, device.battery, device.address);
        }
        println!("current: {:?}", state.current());
    }

    #[test]
    fn kinds_come_from_the_class_and_the_name() {
        assert_eq!(audio_kind("OnePlus Nord Buds 3 Pro", Some(4), Some(1)), Some(DeviceKind::Earbuds));
        assert_eq!(audio_kind("WH-1000XM5", Some(4), Some(6)), Some(DeviceKind::Headphones));
        assert_eq!(audio_kind("JBL Flip 6", Some(4), Some(5)), Some(DeviceKind::Speaker));
        assert_eq!(audio_kind("Galaxy S24", Some(2), Some(3)), None);
        assert_eq!(audio_kind("AirPods Pro", None, None), Some(DeviceKind::Earbuds));
        assert_eq!(normalize_address("08:12:87:e1:2e:64"), "081287E12E64");
    }
}
