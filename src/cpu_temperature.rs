//! One aligned, read-only word from the optional LibreHardwareMonitor tray process.
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};
use windows::core::w;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Memory::{
    MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ,
};

fn unpack(sample: u64, now: u64) -> Option<f32> {
    let timestamp = sample >> 32;
    let hundredths = sample as u32;
    (timestamp != 0 && now.checked_sub(timestamp)? <= 5 && hundredths <= 15000)
        .then_some(hundredths as f32 / 100.0)
}

pub fn current() -> Option<f32> {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    // Read an aligned x64 sample in a single load. Release both borrowed OS
    // resources before returning so exiting or restarting the provider is safe.
    let sample = unsafe {
        let handle = OpenFileMappingW(
            FILE_MAP_READ.0,
            false,
            w!("Local\\OpenDeck.HardwareGraphs.CpuTemperature.v1"),
        )
        .ok()?;
        let view = MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 8);
        let _ = CloseHandle(handle);
        if view.Value.is_null() {
            return None;
        }
        let value = (&*view.Value.cast::<AtomicU64>()).load(Ordering::Relaxed);
        let _ = UnmapViewOfFile(view);
        value
    };
    unpack(sample, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_reading_expires_and_never_becomes_zero_on_disconnect() {
        let reading = (100u64 << 32) | 6250;
        assert_eq!(unpack(reading, 101), Some(62.5));
        assert_eq!(unpack(reading, 106), None);
        assert_eq!(unpack(reading, 99), None);
        assert_eq!(unpack(0, 100), None);
    }
}
