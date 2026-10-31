//! A precision touchpad's fingers, read from its raw reports (`WM_INPUT`),
//! in millimetres on the pad.
//!
//! Windows turns touchpad movement into scroll amounts that are scaled by
//! its settings and accelerated, so a swipe sent that way moved the phone's
//! finger anything from a few pixels to several screens. The raw reports
//! say where each finger is, and the device says how big the pad is
//! (precision touchpads must), so the phone's finger can move exactly as far
//! as the fingers on the pad: the same feel as swiping on the phone.
//!
//! Each finger is a link collection in the report with X, Y (generic desktop
//! page) and a tip switch and contact ID (digitizer page); the report's
//! contact count says how many fingers this frame has. In "hybrid" mode a
//! frame is spread over several reports, later ones with a count of 0.

use std::collections::HashMap;
use std::mem::size_of;

use windows::Win32::Devices::HumanInterfaceDevice::*;
use windows::Win32::Foundation::{HANDLE, LPARAM};
use windows::Win32::UI::Input::*;

const DIGITIZER: u16 = 0x0D;
const GENERIC: u16 = 0x01;
const TIP: u16 = 0x42;
const CONTACT_ID: u16 = 0x51;
const CONTACT_COUNT: u16 = 0x54;
const X: u16 = 0x30;
const Y: u16 = 0x31;

/// A finger touching the pad: its contact ID and position (mm from the pad's corner).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Contact {
    pub id: u32,
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Copy, Debug)]
struct Axis {
    min: i32,
    mm_per_unit: f64,
    /// The pad's size along this axis.
    size_mm: f64,
}

impl Axis {
    /// From the device's description of the value; `None` without usable physical units.
    fn from_caps(c: &HIDP_VALUE_CAPS) -> Option<Self> {
        let logical = f64::from(c.LogicalMax) - f64::from(c.LogicalMin);
        let physical = f64::from(c.PhysicalMax) - f64::from(c.PhysicalMin);
        if logical <= 0.0 || physical <= 0.0 {
            return None;
        }
        // Units: low nibble the system (1 SI linear: cm, 3 English linear:
        // inch), next nibble the power of length (1). Exponent: a signed nibble.
        if (c.Units >> 4) & 0xF != 1 {
            return None;
        }
        let per_unit_mm = match c.Units & 0xF {
            1 => 10.0,
            3 => 25.4,
            _ => return None,
        };
        let e = (c.UnitsExp & 0xF) as i32;
        let exp = if e >= 8 { e - 16 } else { e };
        let mm = physical * 10f64.powi(exp) * per_unit_mm;
        // A touchpad is a few cm to a few tens of cm across.
        if !(10.0..=500.0).contains(&mm) {
            return None;
        }
        Some(Self { min: c.LogicalMin, mm_per_unit: mm / logical, size_mm: mm })
    }

    fn mm(&self, raw: u32) -> f64 {
        f64::from(raw as i32 - self.min) * self.mm_per_unit
    }
}

struct Finger {
    collection: u16,
    x: Axis,
    y: Axis,
}

struct Device {
    /// The device's preparsed report description (opaque; u64 for alignment).
    preparsed: Vec<u64>,
    fingers: Vec<Finger>,
    /// The link collection holding the contact count, if it has one.
    count: Option<u16>,
}

impl Device {
    fn data(&self) -> PHIDP_PREPARSED_DATA {
        PHIDP_PREPARSED_DATA(self.preparsed.as_ptr() as isize)
    }

    fn open(handle: HANDLE) -> Option<Self> {
        unsafe {
            let mut size = 0u32;
            GetRawInputDeviceInfoW(handle, RIDI_PREPARSEDDATA, None, &mut size);
            if size == 0 {
                return None;
            }
            let mut preparsed = vec![0u64; (size as usize).div_ceil(8)];
            if GetRawInputDeviceInfoW(handle, RIDI_PREPARSEDDATA, Some(preparsed.as_mut_ptr().cast()), &mut size)
                == u32::MAX
            {
                return None;
            }
            let data = PHIDP_PREPARSED_DATA(preparsed.as_ptr() as isize);
            let mut caps = HIDP_CAPS::default();
            if HidP_GetCaps(data, &mut caps) != HIDP_STATUS_SUCCESS {
                return None;
            }
            let mut n = caps.NumberInputValueCaps;
            let mut values = vec![HIDP_VALUE_CAPS::default(); n as usize];
            if HidP_GetValueCaps(HidP_Input, values.as_mut_ptr(), &mut n, data) != HIDP_STATUS_SUCCESS {
                return None;
            }
            values.truncate(n as usize);
            let mut axes: HashMap<u16, (Option<Axis>, Option<Axis>)> = HashMap::new();
            let mut count = None;
            for v in &values {
                let (lo, hi) = if v.IsRange.0 != 0 {
                    (v.Anonymous.Range.UsageMin, v.Anonymous.Range.UsageMax)
                } else {
                    (v.Anonymous.NotRange.Usage, v.Anonymous.NotRange.Usage)
                };
                for usage in lo..=hi {
                    match (v.UsagePage, usage) {
                        (GENERIC, X) => axes.entry(v.LinkCollection).or_default().0 = Axis::from_caps(v),
                        (GENERIC, Y) => axes.entry(v.LinkCollection).or_default().1 = Axis::from_caps(v),
                        (DIGITIZER, CONTACT_COUNT) => count = Some(v.LinkCollection),
                        _ => {}
                    }
                }
            }
            let mut fingers: Vec<Finger> = axes
                .into_iter()
                .filter_map(|(collection, a)| Some(Finger { collection, x: a.0?, y: a.1? }))
                .collect();
            fingers.sort_by_key(|f| f.collection);
            if fingers.is_empty() {
                return None;
            }
            let f = &fingers[0];
            log::info!(
                "mirror: touchpad {:.0} x {:.0} mm, {} fingers: swipes go 1:1 to the phone",
                f.x.size_mm,
                f.y.size_mm,
                fingers.len(),
            );
            Some(Self { preparsed, fingers, count })
        }
    }

    fn value(&self, page: u16, collection: u16, usage: u16, report: &[u8]) -> Option<u32> {
        let mut v = 0u32;
        let ok = unsafe { HidP_GetUsageValue(HidP_Input, page, collection, usage, &mut v, self.data(), report) };
        (ok == HIDP_STATUS_SUCCESS).then_some(v)
    }

    fn touching(&self, collection: u16, report: &mut [u8]) -> bool {
        let mut usages = [0u16; 16];
        let mut len = usages.len() as u32;
        let ok = unsafe {
            HidP_GetUsages(HidP_Input, DIGITIZER, collection, usages.as_mut_ptr(), &mut len, self.data(), report)
        };
        ok == HIDP_STATUS_SUCCESS && usages[..len as usize].contains(&TIP)
    }
}

#[derive(Default)]
pub(super) struct Touchpad {
    /// By device handle; `None`: not a touchpad we can measure.
    devices: HashMap<isize, Option<Device>>,
    /// Fingers still to come in the frame being put together.
    expect: usize,
    frame: Vec<Contact>,
}

impl Touchpad {
    /// A touchpad that can be measured has sent reports.
    pub fn works(&self) -> bool {
        self.devices.values().any(Option::is_some)
    }

    /// Reads one `WM_INPUT`. Returns the fingers touching the pad when a
    /// frame (all its fingers) is complete.
    pub fn on_input(&mut self, lp: LPARAM) -> Option<Vec<Contact>> {
        let header = size_of::<RAWINPUTHEADER>() as u32;
        let handle = HRAWINPUT(lp.0 as *mut core::ffi::c_void);
        let mut size = 0u32;
        let mut buf: Vec<u64>;
        unsafe {
            GetRawInputData(handle, RID_INPUT, None, &mut size, header);
            if size < header + 8 {
                return None;
            }
            buf = vec![0u64; (size as usize).div_ceil(8)];
            if GetRawInputData(handle, RID_INPUT, Some(buf.as_mut_ptr().cast()), &mut size, header) == u32::MAX {
                return None;
            }
        }
        let bytes = unsafe { std::slice::from_raw_parts_mut(buf.as_mut_ptr().cast::<u8>(), size as usize) };
        let head: RAWINPUTHEADER = unsafe { std::ptr::read_unaligned(bytes.as_ptr().cast()) };
        if head.dwType != RIM_TYPEHID.0 {
            return None;
        }
        let at = header as usize;
        let u32_at = |i: usize| u32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        let (each, reports) = (u32_at(at) as usize, u32_at(at + 4) as usize);
        let data_at = at + 8;
        if each == 0 || data_at + each * reports > bytes.len() {
            return None;
        }
        let key = head.hDevice.0 as isize;
        let device = self.devices.entry(key).or_insert_with(|| Device::open(head.hDevice));
        let Some(device) = device.as_ref() else { return None };

        let mut done = None;
        for r in 0..reports {
            let report = &mut bytes[data_at + r * each..data_at + (r + 1) * each];
            let count = device.count.and_then(|c| device.value(DIGITIZER, c, CONTACT_COUNT, report));
            let take = match count {
                // A new frame, with this many fingers (some maybe in later reports).
                Some(c) if c > 0 => {
                    self.frame.clear();
                    self.expect = c as usize;
                    self.expect.min(device.fingers.len())
                }
                // The rest of a frame (hybrid mode); nothing expected: a stray report.
                Some(_) => self.expect.min(device.fingers.len()),
                None => {
                    self.frame.clear();
                    self.expect = device.fingers.len();
                    self.expect
                }
            };
            if take == 0 {
                continue;
            }
            for (i, f) in device.fingers.iter().take(take).enumerate() {
                if !device.touching(f.collection, report) {
                    continue;
                }
                let (Some(x), Some(y)) =
                    (device.value(GENERIC, f.collection, X, report), device.value(GENERIC, f.collection, Y, report))
                else {
                    continue;
                };
                let id = device.value(DIGITIZER, f.collection, CONTACT_ID, report).unwrap_or(i as u32);
                self.frame.push(Contact { id, x: f.x.mm(x), y: f.y.mm(y) });
            }
            self.expect -= take;
            if self.expect == 0 {
                done = Some(std::mem::take(&mut self.frame));
            }
        }
        done
    }
}
