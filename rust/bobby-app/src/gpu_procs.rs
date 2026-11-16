// GPU load and dedicated VRAM per process, read from the Windows "GPU Engine" and "GPU Process Memory"
// performance counters (what Task Manager shows). NVML cannot give per-process numbers on a GeForce under WDDM.
use std::collections::HashMap;
use windows::core::w;
use windows::Win32::System::Performance::*;

pub struct GpuProcs {
    query: PDH_HQUERY,
    engine: PDH_HCOUNTER,
    memory: PDH_HCOUNTER,
}

// ponytail: PDH handles are plain pointers used from one task at a time.
unsafe impl Send for GpuProcs {}

/// Per pid: (GPU %, dedicated VRAM in MB).
pub type Usage = HashMap<u32, (f64, f64)>;

impl GpuProcs {
    pub fn open() -> Option<Self> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(None, 0, &mut query) != 0 {
                return None;
            }
            let (mut engine, mut memory) = (PDH_HCOUNTER::default(), PDH_HCOUNTER::default());
            if PdhAddEnglishCounterW(query, w!("\\GPU Engine(*)\\Utilization Percentage"), 0, &mut engine) != 0
                || PdhAddEnglishCounterW(query, w!("\\GPU Process Memory(*)\\Dedicated Usage"), 0, &mut memory) != 0
            {
                PdhCloseQuery(query);
                return None;
            }
            PdhCollectQueryData(query); // utilisation is a rate: the first sample only sets the baseline
            Some(Self { query, engine, memory })
        }
    }

    pub fn sample(&mut self) -> Usage {
        let mut out = Usage::new();
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return out;
        }
        out.extend(engine_load(&read(self.engine)).into_iter().map(|(pid, g)| (pid, (g, 0.0))));
        for (name, bytes) in read(self.memory) {
            if let Some(pid) = pid_of(&name) {
                out.entry(pid).or_default().1 += bytes / 1_048_576.0;
            }
        }
        out
    }
}

impl Drop for GpuProcs {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.query) };
    }
}

fn read(counter: PDH_HCOUNTER) -> Vec<(String, f64)> {
    unsafe {
        let (mut bytes, mut count) = (0u32, 0u32);
        if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, None) != PDH_MORE_DATA {
            return vec![];
        }
        let mut buf = vec![0u64; bytes as usize / 8 + 1]; // items, then their names, 8-byte aligned
        let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        if PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut bytes, &mut count, Some(items)) != 0 {
            return vec![];
        }
        std::slice::from_raw_parts(items, count as usize)
            .iter()
            .filter(|i| i.FmtValue.CStatus == 0)
            .filter_map(|i| Some((i.szName.to_string().ok()?, i.FmtValue.Anonymous.doubleValue)))
            .collect()
    }
}

/// "pid_21144_luid_0x0_0xD1F3_phys_0_eng_0_engtype_3D" -> 21144
fn pid_of(name: &str) -> Option<u32> {
    name.strip_prefix("pid_")?.split('_').next()?.parse().ok()
}

/// Like Task Manager: add up the engines of one type, then keep the busiest type of each process.
fn engine_load(rows: &[(String, f64)]) -> HashMap<u32, f64> {
    let mut by_type: HashMap<(u32, &str), f64> = HashMap::new();
    for (name, v) in rows {
        if let (Some(pid), Some(t)) = (pid_of(name), name.rsplit("engtype_").next()) {
            *by_type.entry((pid, t)).or_default() += v;
        }
    }
    let mut out: HashMap<u32, f64> = HashMap::new();
    for ((pid, _), v) in by_type {
        let e = out.entry(pid).or_default();
        *e = e.max(v.min(100.0));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engines_sum_per_type_then_max() {
        let r = |n: &str, v| (n.to_string(), v);
        let rows = [
            r("pid_7_luid_0x0_0x1_phys_0_eng_0_engtype_3D", 30.0),
            r("pid_7_luid_0x0_0x1_phys_0_eng_1_engtype_3D", 20.0),
            r("pid_7_luid_0x0_0x1_phys_0_eng_2_engtype_Compute_0", 40.0),
            r("pid_9_luid_0x0_0x1_phys_0_eng_0_engtype_Copy", 5.0),
        ];
        let l = engine_load(&rows);
        assert_eq!(l[&7], 50.0);
        assert_eq!(l[&9], 5.0);
        assert_eq!(pid_of("pid_21144_luid_0x00000000_0x0000D1F3_phys_0"), Some(21144));
        assert_eq!(pid_of("_Total"), None);
    }

    #[test]
    fn live_counters_open() {
        let mut g = GpuProcs::open().expect("pdh");
        std::thread::sleep(std::time::Duration::from_millis(500));
        let u = g.sample();
        println!("{} processes on the GPU: {:?}", u.len(), u.iter().take(5).collect::<Vec<_>>());
    }
}
