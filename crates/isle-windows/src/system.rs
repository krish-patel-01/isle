// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 zickrian

//! Charging, battery, network and Bluetooth alerts (isled's system/ on GNOME): the same events, read from Windows.
//! Power comes from GetSystemPowerStatus, the network from Windows' connection profile, Bluetooth from the paired
//! devices Windows knows, with each one's battery as Windows Settings shows it.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;

/// How often the state is read, besides whenever Windows says a device or the network changed.
const TICK: Duration = Duration::from_secs(2);
/// How often while something is about to be told (a device connecting, the network dropping).
const QUICK: Duration = Duration::from_millis(250);
/// Offline this long, so a connection that blinks (changing access points) says nothing.
const OFFLINE_HOLD: Duration = Duration::from_millis(2500);
/// Plugged in and out again within this alerts only once (a charge limit can make the charger seem to come and go).
const FLAP_WINDOW: Duration = Duration::from_secs(30);
/// Percentages at which a low-battery warning is shown, once each.
const LOW_LEVELS: [f64; 3] = [20.0, 10.0, 5.0];
/// Headsets report their battery a moment after connecting; a device that bounces within this is not announced.
const BATTERY_GRACE: Duration = Duration::from_millis(1200);
/// A tick this late means the computer slept: the network dropping and coming back then is not news.
const SLEPT: Duration = Duration::from_secs(15);

/// A connected Bluetooth device, as the island's alert shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BluetoothDevice {
    pub path: String,
    pub name: String,
    /// One of Isle's icons, by the kind of device.
    pub icon: String,
    pub battery: Option<u8>,
}

/// A change worth an alert, tagged by `kind` as isled's are.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SystemEvent {
    Power { plugged: bool, percent: f64 },
    BatteryLow { percent: f64 },
    BatteryFull,
    Network { connected: bool, connection: String, name: String },
    Bluetooth { connected: bool, device: BluetoothDevice },
}

/// The battery as read.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Power {
    pub percent: f64,
    pub plugged: bool,
}

/// Follows the power, the network and Bluetooth for good, calling `alert` with each change worth showing.
pub fn watch(alert: impl Fn(SystemEvent)) {
    imp::init();
    // Windows wakes the loop when a paired device or the network changes, so those are told at once.
    let (wake, woken) = std::sync::mpsc::channel();
    imp::on_change(wake);
    let mut power = PowerWatch::default();
    let mut network: Option<bool> = None;
    let mut offline_since: Option<Instant> = None;
    let mut bluetooth = BluetoothWatch::default();
    let mut last = SystemTime::now();
    loop {
        let now = SystemTime::now();
        let slept = now.duration_since(last).unwrap_or_default() > SLEPT;
        last = now;
        if slept {
            log::info!(target: "isle::system", "woke from sleep");
            network = None;
            offline_since = None;
        }

        if let Some(event) = imp::power().and_then(|reading| power.next(reading, Instant::now())) {
            alert(event);
        }

        // Offline for a moment, so a connection that blinks (changing access points) says nothing; and back online.
        let (connected, connection, name) = imp::network();
        match (network, connected) {
            (Some(true), false) => {
                let since = *offline_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= OFFLINE_HOLD {
                    alert(SystemEvent::Network { connected, connection, name });
                    network = Some(false);
                    offline_since = None;
                }
            }
            (Some(false), true) => {
                alert(SystemEvent::Network { connected, connection, name });
                network = Some(true);
            }
            _ => {
                network = Some(connected);
                offline_since = None;
            }
        }

        for event in bluetooth.next(imp::bluetooth(), Instant::now()) {
            alert(event);
        }
        let wait = if bluetooth.waiting.is_empty() && offline_since.is_none() { TICK } else { QUICK };
        let _ = woken.recv_timeout(wait);
        while woken.try_recv().is_ok() {}
    }
}

/// The plugged, full and low alerts, from one reading to the next.
#[derive(Default)]
struct PowerWatch {
    previous: Option<Power>,
    last_power_alert: Option<Instant>,
    /// The lowest level warned about since the charger was unplugged.
    warned: Option<f64>,
}

impl PowerWatch {
    fn next(&mut self, now: Power, at: Instant) -> Option<SystemEvent> {
        let event = match self.previous {
            Some(before) if before.plugged != now.plugged => {
                let flapping = self.last_power_alert.is_some_and(|then| at.duration_since(then) < FLAP_WINDOW);
                self.last_power_alert = Some(at);
                (!flapping).then_some(SystemEvent::Power { plugged: now.plugged, percent: now.percent })
            }
            Some(before) if now.plugged && before.percent < 100.0 && now.percent >= 100.0 => Some(SystemEvent::BatteryFull),
            Some(before) if !now.plugged => low_level(before.percent, now.percent, self.warned).map(|level| {
                self.warned = Some(level);
                SystemEvent::BatteryLow { percent: now.percent }
            }),
            _ => None,
        };
        if now.plugged {
            self.warned = None;
        }
        self.previous = Some(now);
        event
    }
}

/// The low level the battery just went under, if not warned about since unplugged.
fn low_level(before: f64, now: f64, warned: Option<f64>) -> Option<f64> {
    LOW_LEVELS
        .into_iter()
        .filter(|&level| warned.is_none_or(|warned| level < warned))
        .rfind(|&level| before > level && now <= level)
}

/// Devices connecting and going: a connection is told once it has held for [`BATTERY_GRACE`].
#[derive(Default)]
struct BluetoothWatch {
    started: bool,
    /// Connected and told, by id.
    told: HashMap<String, BluetoothDevice>,
    /// Connected since, not yet told.
    waiting: HashMap<String, Instant>,
}

/// A paired device as listed: its id, name, kind of icon and whether it is connected now.
#[derive(Debug, Clone)]
pub struct Paired {
    pub id: String,
    pub name: String,
    pub icon: &'static str,
    pub connected: bool,
    /// Its container, for the battery.
    pub container: String,
}

impl BluetoothWatch {
    fn next(&mut self, paired: Option<Vec<Paired>>, at: Instant) -> Vec<SystemEvent> {
        let Some(paired) = paired else {
            return Vec::new();
        };
        let connected: HashMap<&str, &Paired> = paired.iter().filter(|p| p.connected).map(|p| (p.id.as_str(), p)).collect();
        // What was connected when Isle started is not news.
        if !self.started {
            self.started = true;
            for device in connected.values() {
                self.told.insert(device.id.clone(), device_of(device, None));
            }
            return Vec::new();
        }
        let mut events = Vec::new();
        let gone: Vec<String> = self.told.keys().filter(|id| !connected.contains_key(id.as_str())).cloned().collect();
        for id in gone {
            if let Some(device) = self.told.remove(&id) {
                events.push(SystemEvent::Bluetooth { connected: false, device: BluetoothDevice { battery: None, ..device } });
            }
        }
        self.waiting.retain(|id, _| connected.contains_key(id.as_str()));
        for (id, device) in &connected {
            if self.told.contains_key(*id) {
                continue;
            }
            let since = *self.waiting.entry((*id).to_owned()).or_insert(at);
            if at.duration_since(since) >= BATTERY_GRACE {
                self.waiting.remove(*id);
                let shown = device_of(device, imp::battery(&device.container));
                self.told.insert((*id).to_owned(), shown.clone());
                events.push(SystemEvent::Bluetooth { connected: true, device: shown });
            }
        }
        events
    }
}

fn device_of(paired: &Paired, battery: Option<u8>) -> BluetoothDevice {
    BluetoothDevice { path: paired.id.clone(), name: paired.name.clone(), icon: paired.icon.to_owned(), battery }
}

/// Isle's icon for a Bluetooth device, from its Class of Device (major and minor) or else its name.
fn icon_for(major: Option<u32>, minor: Option<u32>, name: &str) -> &'static str {
    let name = name.to_lowercase();
    // Soundbars and speakers often say they are headsets.
    if major.is_none_or(|major| major == 4) && ["speaker", "soundbar", " bar", "boom", "flip"].iter().any(|word| name.contains(word)) {
        return "isle-speaker-device-symbolic";
    }
    match (major, minor) {
        (Some(1), _) => return "isle-laptop-symbolic",
        (Some(2), _) => return "isle-smartphone-symbolic",
        (Some(4), Some(minor)) if [5, 10].contains(&minor) => return "isle-speaker-device-symbolic",
        (Some(4), _) => return "isle-headphones-symbolic",
        (Some(5), Some(minor)) if minor & 0x0F == 1 || minor & 0x0F == 2 => return "isle-gamepad-symbolic",
        (Some(5), Some(minor)) if minor & 0x10 != 0 => return "isle-keyboard-symbolic",
        (Some(5), Some(minor)) if minor & 0x20 != 0 => return "isle-mouse-symbolic",
        _ => {}
    }
    let has = |words: &[&str]| words.iter().any(|word| name.contains(word));
    if has(&["bud", "pods", "headphone", "headset", "earphone", "wh-", "wf-", "jabra", "bose", "beats"]) {
        "isle-headphones-symbolic"
    } else if has(&["speaker", "soundbar", "boom", "flip", "charge "]) {
        "isle-speaker-device-symbolic"
    } else if has(&["mouse", "mx master", "trackball"]) {
        "isle-mouse-symbolic"
    } else if has(&["keyboard", "keys"]) {
        "isle-keyboard-symbolic"
    } else if has(&["controller", "gamepad", "xbox", "dualsense", "dualshock", "joy-con"]) {
        "isle-gamepad-symbolic"
    } else if has(&["phone", "galaxy", "pixel", "redmi", "oneplus"]) {
        "isle-smartphone-symbolic"
    } else {
        "isle-bluetooth-symbolic"
    }
}

#[cfg(windows)]
mod imp {
    use super::{Paired, Power, icon_for};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::mpsc::Sender;

    use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};

    use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind};
    use windows::Foundation::TypedEventHandler;
    use windows::Networking::Connectivity::NetworkStatusChangedEventHandler;
    use windows_collections::{IIterable, IMapView};
    use windows::Foundation::IPropertyValue;
    use windows::Networking::Connectivity::{NetworkConnectivityLevel, NetworkInformation};
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    use windows::core::{HSTRING, IInspectable, Interface};

    /// The device's container, which its battery is read from (as a device, and as an association endpoint).
    const CONTAINER: &str = "System.Devices.ContainerId";
    const AEP_CONTAINER: &str = "System.Devices.Aep.ContainerId";
    /// DEVPKEY_Bluetooth_Battery: what Windows Settings shows beside a device.
    const BATTERY: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";

    pub fn init() {
        // SAFETY: joins this thread to the multithreaded apartment, for WinRT.
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    }

    /// A paired device opened as Bluetooth itself: it says the moment it connects or drops, and tells as it does.
    enum Link {
        Classic(BluetoothDevice),
        LowEnergy(BluetoothLEDevice),
    }

    thread_local! {
        /// Each device opened once (None: it couldn't be), by id; all on the watch's thread.
        static LINKS: RefCell<HashMap<String, Option<Link>>> = RefCell::new(HashMap::new());
        /// Where a device's connection changing wakes the watch.
        static WAKE: RefCell<Option<Sender<()>>> = const { RefCell::new(None) };
    }

    fn open(id: &str, low_energy: bool) -> Option<Link> {
        let id = HSTRING::from(id);
        let wake = WAKE.with(|wake| wake.borrow().clone());
        if low_energy {
            let device = BluetoothLEDevice::FromIdAsync(&id).and_then(|op| op.get()).ok()?;
            if let Some(wake) = wake {
                let _ = device.ConnectionStatusChanged(&TypedEventHandler::<BluetoothLEDevice, IInspectable>::new(move |_, _| {
                    let _ = wake.send(());
                    Ok(())
                }));
            }
            return Some(Link::LowEnergy(device));
        }
        let device = BluetoothDevice::FromIdAsync(&id).and_then(|op| op.get()).ok()?;
        if let Some(wake) = wake {
            let _ = device.ConnectionStatusChanged(&TypedEventHandler::<BluetoothDevice, IInspectable>::new(move |_, _| {
                let _ = wake.send(());
                Ok(())
            }));
        }
        Some(Link::Classic(device))
    }

    /// Whether the device is connected, and its class (major, minor) when it is a Classic one.
    fn linked(id: &str, low_energy: bool) -> Option<(bool, Option<u32>, Option<u32>)> {
        LINKS.with(|links| {
            let mut links = links.borrow_mut();
            let link = links.entry(id.to_owned()).or_insert_with(|| open(id, low_energy));
            match link.as_ref()? {
                Link::Classic(device) => {
                    let connected = device.ConnectionStatus().ok()? == BluetoothConnectionStatus::Connected;
                    let class = device.ClassOfDevice().ok();
                    let major = class.as_ref().and_then(|c| c.MajorClass().ok()).map(|m| m.0 as u32);
                    let minor = class.as_ref().and_then(|c| c.MinorClass().ok()).map(|m| m.0 as u32);
                    Some((connected, major, minor))
                }
                Link::LowEnergy(device) => {
                    Some((device.ConnectionStatus().ok()? == BluetoothConnectionStatus::Connected, None, None))
                }
            }
        })
    }

    /// Sends on `wake` whenever a paired Bluetooth device connects or disconnects (once it has been read), or the
    /// network changes.
    pub fn on_change(wake: Sender<()>) {
        WAKE.with(|slot| *slot.borrow_mut() = Some(wake.clone()));
        let _ = NetworkInformation::NetworkStatusChanged(&NetworkStatusChangedEventHandler::new(move |_| {
            let _ = wake.send(());
            Ok(())
        }));
    }

    /// The battery, or None on a computer without one.
    pub fn power() -> Option<Power> {
        let mut status = SYSTEM_POWER_STATUS::default();
        // SAFETY: fills the struct given.
        unsafe { GetSystemPowerStatus(&mut status) }.ok()?;
        if status.BatteryFlag & 128 != 0 || status.BatteryFlag == 255 || status.BatteryLifePercent > 100 {
            return None;
        }
        Some(Power { percent: f64::from(status.BatteryLifePercent), plugged: status.ACLineStatus == 1 })
    }

    /// Whether Windows has a connection, its kind and its name.
    pub fn network() -> (bool, String, String) {
        let read = || -> windows::core::Result<(bool, String, String)> {
            let Ok(profile) = NetworkInformation::GetInternetConnectionProfile() else {
                return Ok((false, String::new(), String::new()));
            };
            let level = profile.GetNetworkConnectivityLevel()?;
            let connected = level != NetworkConnectivityLevel::None;
            let kind = if profile.IsWlanConnectionProfile().unwrap_or(false) {
                "wifi"
            } else if profile.IsWwanConnectionProfile().unwrap_or(false) {
                "other"
            } else {
                "ethernet"
            };
            Ok((connected, kind.to_owned(), profile.ProfileName().map(|name| name.to_string()).unwrap_or_default()))
        };
        read().unwrap_or((true, String::new(), String::new()))
    }

    fn property(map: &IMapView<HSTRING, IInspectable>, key: &str) -> Option<IPropertyValue> {
        map.Lookup(&HSTRING::from(key)).ok()?.cast::<IPropertyValue>().ok()
    }

    /// The paired Bluetooth devices, or None when Windows couldn't say. Read from Windows' own list of paired
    /// devices, which answers at once: a search of association endpoints scans the air first (30 seconds).
    pub fn bluetooth() -> Option<Vec<Paired>> {
        let wanted: Vec<HSTRING> = vec![HSTRING::from(CONTAINER), HSTRING::from(AEP_CONTAINER)];
        let wanted: IIterable<HSTRING> = wanted.into();
        let selectors = [
            (BluetoothDevice::GetDeviceSelectorFromPairingState(true).ok()?, false),
            (BluetoothLEDevice::GetDeviceSelectorFromPairingState(true).ok()?, true),
        ];
        let mut devices = Vec::new();
        for (selector, low_energy) in selectors {
            let Ok(found) = DeviceInformation::FindAllAsyncAqsFilterAndAdditionalProperties(&selector, &wanted)
                .and_then(|op| op.get())
            else {
                continue;
            };
            for info in found {
                let Ok(id) = info.Id().map(|id| id.to_string()) else { continue };
                let name = info.Name().map(|name| name.to_string()).unwrap_or_default();
                let Some((connected, major, minor)) = linked(&id, low_energy) else { continue };
                let container = info
                    .Properties()
                    .ok()
                    .and_then(|map| property(&map, CONTAINER).or_else(|| property(&map, AEP_CONTAINER)))
                    .and_then(|value| value.GetGuid().ok())
                    .map(|guid| format!("{{{guid:?}}}"))
                    .unwrap_or_default();
                // Earbuds can be paired both ways: one entry per device (its Classic one, read first), connected
                // if either is.
                if let Some(seen) = devices.iter_mut().find(|d: &&mut Paired| !container.is_empty() && d.container == container) {
                    seen.connected |= connected;
                    continue;
                }
                devices.push(Paired { id, icon: icon_for(major, minor, &name), name, connected, container });
            }
        }
        Some(devices)
    }

    /// A device's battery, from any of the device nodes in its container.
    pub fn battery(container: &str) -> Option<u8> {
        if container.is_empty() {
            return None;
        }
        let wanted: Vec<HSTRING> = vec![HSTRING::from(BATTERY)];
        let wanted: IIterable<HSTRING> = wanted.into();
        let filter = format!("System.Devices.ContainerId:=\"{container}\"");
        let found = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(
            &HSTRING::from(filter),
            &wanted,
            DeviceInformationKind::Device,
        )
        .ok()?
        .get()
        .ok()?;
        found.into_iter().find_map(|info| {
            let map = info.Properties().ok()?;
            property(&map, BATTERY)?.GetUInt8().ok().filter(|&percent| percent <= 100)
        })
    }
}

#[cfg(not(windows))]
mod imp {
    use super::{Paired, Power};
    pub fn init() {}
    pub fn on_change(_wake: std::sync::mpsc::Sender<()>) {}
    pub fn power() -> Option<Power> {
        None
    }
    pub fn network() -> (bool, String, String) {
        (true, String::new(), String::new())
    }
    pub fn bluetooth() -> Option<Vec<Paired>> {
        None
    }
    pub fn battery(_container: &str) -> Option<u8> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(percent: f64, plugged: bool) -> Power {
        Power { percent, plugged }
    }

    #[test]
    fn unplugging_low_and_full_alert_once_each() {
        let mut watch = PowerWatch::default();
        let start = Instant::now();
        assert!(watch.next(reading(50.0, true), start).is_none());
        assert!(matches!(watch.next(reading(50.0, false), start), Some(SystemEvent::Power { plugged: false, .. })));
        assert!(matches!(watch.next(reading(20.0, false), start), Some(SystemEvent::BatteryLow { .. })));
        // Read a little up and down again: warned once.
        assert!(watch.next(reading(21.0, false), start).is_none());
        assert!(watch.next(reading(20.0, false), start).is_none());
        assert!(matches!(watch.next(reading(10.0, false), start), Some(SystemEvent::BatteryLow { .. })));
        // Plugged in again within the flap window: not told twice.
        assert!(watch.next(reading(10.0, true), start + Duration::from_secs(5)).is_none());
        assert!(watch.next(reading(99.0, true), start).is_none());
        assert!(matches!(watch.next(reading(100.0, true), start), Some(SystemEvent::BatteryFull)));
    }

    fn paired(id: &str, connected: bool) -> Paired {
        Paired { id: id.into(), name: "Buds".into(), icon: "isle-headphones-symbolic", connected, container: String::new() }
    }

    #[test]
    fn a_connection_is_told_after_it_holds() {
        let mut watch = BluetoothWatch::default();
        let start = Instant::now();
        assert!(watch.next(Some(vec![paired("a", true), paired("b", false)]), start).is_empty());
        assert!(watch.next(Some(vec![paired("a", true), paired("b", true)]), start).is_empty());
        let told = watch.next(Some(vec![paired("a", true), paired("b", true)]), start + BATTERY_GRACE);
        assert!(matches!(&told[..], [SystemEvent::Bluetooth { connected: true, device }] if device.path == "b"));
        let gone = watch.next(Some(vec![paired("a", false), paired("b", true)]), start + BATTERY_GRACE);
        assert!(matches!(&gone[..], [SystemEvent::Bluetooth { connected: false, device }] if device.path == "a"));
    }

    #[test]
    fn devices_get_their_kind_of_icon() {
        assert_eq!(icon_for(Some(4), Some(1), "x"), "isle-headphones-symbolic");
        assert_eq!(icon_for(Some(5), Some(0x20), "x"), "isle-mouse-symbolic");
        assert_eq!(icon_for(None, None, "Galaxy Buds2 Pro"), "isle-headphones-symbolic");
        assert_eq!(icon_for(None, None, "Thing"), "isle-bluetooth-symbolic");
    }
}

#[cfg(all(test, windows))]
mod live {
    /// What this computer says now: `cargo test --release live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn time_the_readings() {
        super::imp::init();
        for round in 0..3 {
            let at = std::time::Instant::now();
            let _ = super::imp::power();
            let power = at.elapsed();
            let at = std::time::Instant::now();
            let _ = super::imp::network();
            let network = at.elapsed();
            let at = std::time::Instant::now();
            let devices = super::imp::bluetooth().unwrap_or_default();
            let bluetooth = at.elapsed();
            let at = std::time::Instant::now();
            for device in &devices {
                let _ = super::imp::battery(&device.container);
            }
            println!("round {round}: power {power:?} network {network:?} bluetooth {bluetooth:?} batteries {:?}", at.elapsed());
        }
    }

    #[test]
    #[ignore]
    fn read_this_computer() {
        super::imp::init();
        println!("power {:?}", super::imp::power());
        println!("network {:?}", super::imp::network());
        for device in super::imp::bluetooth().unwrap_or_default() {
            println!("bt {device:?} battery {:?}", super::imp::battery(&device.container));
        }
    }
}
