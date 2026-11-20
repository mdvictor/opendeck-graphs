//! A single sampler owns the PDH rate history. WMI covers warm-up or unavailable groups.
use std::{
    ptr,
    time::{Duration, Instant},
};
use windows_sys::{
    core::{w, PCWSTR},
    Win32::System::Performance::*,
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub struct Groups {
    pub cpu: bool,
    pub network: bool,
}
#[derive(Default)]
pub struct Rates {
    pub cpu: Option<f32>,
    pub network: Option<(u64, u64)>,
}
impl Rates {
    fn missing(&self, groups: Groups) -> bool {
        (groups.cpu && self.cpu.is_none()) || (groups.network && self.network.is_none())
    }
}

struct Query {
    handle: PDH_HQUERY,
    cpu: PDH_HCOUNTER,
    rx: PDH_HCOUNTER,
    tx: PDH_HCOUNTER,
    // u64 alignment covers PDH_FMT_COUNTERVALUE_ITEM_W on Windows x64.
    scratch: Vec<u64>,
    primed: bool,
}
// PDH queries are not apartment-bound. Exclusive ownership moves between blocking
// pool workers; the sampler never calls into one query concurrently.
unsafe impl Send for Query {}
impl Drop for Query {
    fn drop(&mut self) {
        unsafe {
            PdhCloseQuery(self.handle);
        }
    }
}
impl Query {
    fn open(groups: Groups) -> Option<Self> {
        let mut handle = ptr::null_mut();
        if unsafe { PdhOpenQueryW(ptr::null(), 0, &mut handle) } != 0 {
            return None;
        }
        let mut q = Self {
            handle,
            cpu: ptr::null_mut(),
            rx: ptr::null_mut(),
            tx: ptr::null_mut(),
            scratch: Vec::new(),
            primed: false,
        };
        let add = |path: PCWSTR| {
            let mut h = ptr::null_mut();
            if unsafe { PdhAddEnglishCounterW(handle, path, 0, &mut h) } == 0 {
                h
            } else {
                ptr::null_mut()
            }
        };
        if groups.cpu {
            q.cpu = add(w!(r"\Processor(_Total)\% Processor Time"));
        }
        if groups.network {
            q.rx = add(w!(r"\Network Interface(*)\Bytes Received/sec"));
            q.tx = add(w!(r"\Network Interface(*)\Bytes Sent/sec"));
        }
        Some(q)
    }
    fn read(&mut self) -> Option<Rates> {
        if unsafe { PdhCollectQueryData(self.handle) } != 0 {
            return None;
        }
        if !std::mem::replace(&mut self.primed, true) {
            return Some(Rates::default());
        }
        let cpu = cpu_value(self.cpu);
        let network =
            sum_rates(self.rx, &mut self.scratch).zip(sum_rates(self.tx, &mut self.scratch));
        Some(Rates { cpu, network })
    }
}
fn number(v: &PDH_FMT_COUNTERVALUE) -> Option<f64> {
    if !matches!(v.CStatus, PDH_CSTATUS_VALID_DATA | PDH_CSTATUS_NEW_DATA) {
        return None;
    }
    let n = unsafe { v.Anonymous.doubleValue };
    (n.is_finite() && n >= 0.0).then_some(n)
}
fn cpu_value(h: PDH_HCOUNTER) -> Option<f32> {
    let mut v = PDH_FMT_COUNTERVALUE::default();
    if h.is_null()
        || unsafe { PdhGetFormattedCounterValue(h, PDH_FMT_DOUBLE, ptr::null_mut(), &mut v) } != 0
    {
        return None;
    }
    number(&v).map(|n| n as f32)
}
fn sum_rates(h: PDH_HCOUNTER, buf: &mut Vec<u64>) -> Option<u64> {
    if h.is_null() {
        return None;
    }
    let (mut bytes, mut count) = (0, 0);
    if unsafe {
        PdhGetFormattedCounterArrayW(h, PDH_FMT_DOUBLE, &mut bytes, &mut count, ptr::null_mut())
    } != PDH_MORE_DATA
    {
        return None;
    }
    // An implausibly large result falls back to WMI; interface churn cannot grow this indefinitely.
    if bytes == 0 || bytes > 1024 * 1024 {
        return None;
    }
    buf.resize((bytes as usize).div_ceil(8), 0);
    let items = buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
    if unsafe { PdhGetFormattedCounterArrayW(h, PDH_FMT_DOUBLE, &mut bytes, &mut count, items) }
        != 0
        || count == 0
        || count as usize > buf.len() * 8 / size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>()
    {
        return None;
    }
    // Pointers to names point into this buffer. Only the numeric items are used.
    let rows = unsafe { std::slice::from_raw_parts(items, count as usize) };
    let sum = rows
        .iter()
        .try_fold(0., |total, row| Some(total + number(&row.FmtValue)?))?;
    Some(sum.round() as u64)
}

#[derive(Default)]
pub struct Sampler {
    query: Option<Query>,
    groups: Groups,
    last: Option<Instant>,
    retry: Option<Instant>,
}
impl Sampler {
    fn prepare(&mut self, groups: Groups, now: Instant) {
        let gap = self
            .last
            .replace(now)
            .is_some_and(|last| now.duration_since(last) > Duration::from_secs(5));
        let retry_due = self.retry.is_some_and(|deadline| now >= deadline);
        if groups != self.groups || retry_due || (gap && self.query.is_some()) {
            self.query = None;
            self.retry = None;
        }
        self.groups = groups;
        if self.query.is_none() && self.retry.is_none() {
            self.query = Query::open(groups);
        }
    }

    pub fn read(&mut self, groups: Groups) -> Rates {
        if groups == Groups::default() {
            *self = Self::default();
            return Rates::default();
        }
        let now = Instant::now();
        self.prepare(groups, now);
        let was_primed = self.query.as_ref().is_some_and(|q| q.primed);
        let Some(readings) = self.query.as_mut().and_then(Query::read) else {
            self.query = None;
            self.retry.get_or_insert(now + Duration::from_secs(30));
            return Rates::default();
        };
        if was_primed && readings.missing(groups) {
            self.retry.get_or_insert(now + Duration::from_secs(30));
        } else {
            self.retry = None;
        }
        readings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn zero_is_a_valid_rate_but_invalid_counter_status_is_unavailable() {
        let mut v = PDH_FMT_COUNTERVALUE::default();
        assert_eq!(number(&v), Some(0.));
        v.CStatus = PDH_CSTATUS_INVALID_DATA;
        assert!(number(&v).is_none());
    }
    #[test]
    #[ignore = "requires live Windows performance counters; run in an isolated test process"]
    fn persistent_query_and_retirement_release_handles() {
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
        fn private_bytes() -> usize {
            use windows_sys::Win32::System::ProcessStatus::{
                K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX,
            };
            let size = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            let mut counters = PROCESS_MEMORY_COUNTERS_EX {
                cb: size,
                ..Default::default()
            };
            assert_ne!(
                unsafe {
                    K32GetProcessMemoryInfo(GetCurrentProcess(), (&raw mut counters).cast(), size)
                },
                0
            );
            counters.PrivateUsage
        }
        fn handles() -> u32 {
            let mut n = 0;
            assert_ne!(
                unsafe { GetProcessHandleCount(GetCurrentProcess(), &mut n) },
                0
            );
            n
        }
        let groups = Groups {
            cpu: true,
            network: true,
        };
        let mut sampler = Sampler::default();
        for _ in 0..3 {
            let _ = sampler.read(groups);
            std::thread::sleep(Duration::from_secs(1));
        }
        let first = handles();
        let before_bytes = private_bytes();
        let read = sampler.read(groups);
        assert!(read.cpu.is_some() && read.network.is_some());
        for _ in 0..32 {
            std::thread::sleep(Duration::from_millis(50));
            let _ = sampler.read(groups);
        }
        let steady = handles();
        let steady_bytes = private_bytes();
        sampler.read(Groups::default());
        assert!(sampler.query.is_none());
        let retired = handles();
        let mut batches = Vec::new();
        for _ in 0..2 {
            for _ in 0..100 {
                let warm = sampler.read(groups);
                assert!(warm.cpu.is_none() && warm.network.is_none());
                assert!(sampler.query.is_some());
                sampler.read(Groups::default());
                assert!(sampler.query.is_none());
            }
            batches.push((handles(), private_bytes()));
        }
        let final_count = handles();
        println!("handles open={first}, after steady reads={steady}, retired={retired}; private bytes open={before_bytes}, after steady reads={steady_bytes}; (handles, private bytes) after 100 and 200 open/close: {batches:?}");
        assert!(steady <= first + 4 && final_count <= retired + 4);
    }
}
