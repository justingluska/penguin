//! How hard the background indexer may work right now.
//!
//! macOS exposes three signals, all cheap enough to read between batches
//! (no notifications needed):
//!
//! - `NSProcessInfo.thermalState` (macOS 10.10.3+). Apple's guidance for
//!   `.fair` is to "reduce or defer background work, like … updating
//!   database indexes"; at `.serious` the fans are at full speed.
//! - `NSProcessInfo.isLowPowerModeEnabled` (macOS 12+): the user asked the
//!   Mac to save energy.
//! - IOKit `IOPSGetProvidingPowerSourceType` ("AC Power" vs "Battery
//!   Power"): Macs without a battery always report AC.
//!
//! Policy (docs/SEMANTIC.md): full pace on AC and cool; a quarter of the
//! time when warm; a fifth on battery; paused in Low Power Mode or when hot.
//! Other platforms report "AC, nominal".

/// What the indexer may do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Budget {
    /// Work this share of the time (0 < duty ≤ 1), sleeping the rest.
    Run { duty: f32 },
    /// Don't work; say why.
    Pause(&'static str),
}

/// Share of wall time spent embedding on AC power with the Mac cool. Not
/// 1.0: leave the CPU some slack so a long backfill never makes the fans
/// the most noticeable thing about Penguin.
const DUTY_AC: f32 = 0.8;
const DUTY_WARM: f32 = 0.25;
const DUTY_BATTERY: f32 = 0.2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Signals {
    pub on_battery: bool,
    pub low_power: bool,
    /// 0 nominal, 1 fair, 2 serious, 3 critical.
    pub thermal: i64,
}

pub fn budget(s: Signals) -> Budget {
    if s.low_power {
        return Budget::Pause("Low Power Mode is on");
    }
    if s.thermal >= 2 {
        return Budget::Pause("the Mac is running hot");
    }
    let mut duty = if s.on_battery { DUTY_BATTERY } else { DUTY_AC };
    if s.thermal == 1 {
        duty = duty.min(DUTY_WARM);
    }
    Budget::Run { duty }
}

pub fn read() -> Signals {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::NSProcessInfo;
        let info = NSProcessInfo::processInfo();
        Signals {
            on_battery: on_battery(),
            low_power: info.isLowPowerModeEnabled(),
            thermal: info.thermalState().0 as i64,
        }
    }
    #[cfg(not(target_os = "macos"))]
    Signals { on_battery: false, low_power: false, thermal: 0 }
}

#[cfg(target_os = "macos")]
fn on_battery() -> bool {
    use std::ffi::{c_char, c_void, CStr};
    type CFTypeRef = *const c_void;
    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        fn IOPSCopyPowerSourcesInfo() -> CFTypeRef;
        fn IOPSGetProvidingPowerSourceType(snapshot: CFTypeRef) -> CFTypeRef;
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRelease(cf: CFTypeRef);
        fn CFStringGetCString(s: CFTypeRef, buf: *mut c_char, len: isize, encoding: u32) -> u8;
    }
    const UTF8: u32 = 0x0800_0100;
    // SAFETY: documented IOKit/CoreFoundation calls. The snapshot is ours
    // (Copy rule) and released; the type string follows the Get rule.
    unsafe {
        let snapshot = IOPSCopyPowerSourcesInfo();
        if snapshot.is_null() {
            return false;
        }
        let kind = IOPSGetProvidingPowerSourceType(snapshot);
        let mut buf = [0 as c_char; 64];
        let battery = !kind.is_null()
            && CFStringGetCString(kind, buf.as_mut_ptr(), buf.len() as isize, UTF8) != 0
            && CStr::from_ptr(buf.as_ptr()).to_bytes() == b"Battery Power";
        CFRelease(snapshot);
        battery
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(on_battery: bool, low_power: bool, thermal: i64) -> Signals {
        Signals { on_battery, low_power, thermal }
    }

    #[test]
    fn policy() {
        assert_eq!(budget(s(false, false, 0)), Budget::Run { duty: DUTY_AC });
        assert_eq!(budget(s(true, false, 0)), Budget::Run { duty: DUTY_BATTERY });
        assert_eq!(budget(s(false, false, 1)), Budget::Run { duty: DUTY_WARM });
        assert_eq!(budget(s(true, false, 1)), Budget::Run { duty: DUTY_BATTERY });
        assert!(matches!(budget(s(false, true, 0)), Budget::Pause(_)));
        assert!(matches!(budget(s(false, false, 2)), Budget::Pause(_)));
        assert!(matches!(budget(s(true, false, 3)), Budget::Pause(_)));
        // Whatever this machine reports, it reads without panicking.
        let _ = budget(read());
    }
}
