// Samples system, GPU and per-part load every 2 s by direct system calls (sysinfo, NVML): no PowerShell, no nvidia-smi.
// Keeps one averaged row per 10 s in _logs/metrics.db (same table as the Node app), 30 days. Publishes `metrics` (live sample).
use crate::supervisor::{now_ms, Bus, Event, Supervisor};
use rusqlite::Connection;
use serde_json::{json, Map, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

pub const TICK: Duration = Duration::from_secs(2);
pub const ROW_EVERY_MS: i64 = 10_000;
pub const KEEP_MS: i64 = 30 * 24 * 3600 * 1000;
pub const COLS: [&str; 13] = [
    "cpu", "ram_used", "ram_total", "gpu", "vram_used", "vram_total", "gpu_temp", "llm_cpu", "llm_ram", "voice_cpu", "voice_ram", "bot_cpu",
    "bot_ram",
];
const MB: f64 = 1_048_576.0;

pub type Db = Arc<Mutex<Connection>>;

fn round(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let db = Connection::open(path)?;
    init(&db)?;
    Ok(db)
}

pub fn init(db: &Connection) -> rusqlite::Result<()> {
    let cols = COLS.iter().map(|c| format!("{c} REAL")).collect::<Vec<_>>().join(", ");
    db.execute_batch(&format!(
        "CREATE TABLE IF NOT EXISTS metrics(ts INTEGER, {cols}); CREATE INDEX IF NOT EXISTS metrics_ts ON metrics(ts);"
    ))?;
    prune(db)
}

fn prune(db: &Connection) -> rusqlite::Result<()> {
    db.execute("DELETE FROM metrics WHERE ts < ?1", [now_ms() - KEEP_MS]).map(|_| ())
}

/// Sums of the 2 s samples since the last stored row, per column (missing values are skipped, like the Node app).
#[derive(Default)]
pub struct Acc([(f64, u32); 13]);

impl Acc {
    pub fn add(&mut self, row: &[Option<f64>; 13]) {
        for (a, v) in self.0.iter_mut().zip(row) {
            if let Some(v) = v {
                a.0 += v;
                a.1 += 1;
            }
        }
    }
    pub fn take(&mut self) -> [Option<f64>; 13] {
        let out = self.0.map(|(s, n)| (n > 0).then(|| s / n as f64));
        *self = Acc::default();
        out
    }
}

pub fn insert(db: &Connection, ts: i64, row: &[Option<f64>; 13]) -> rusqlite::Result<()> {
    let marks = "?, ".repeat(13);
    let mut stmt = db.prepare_cached(&format!("INSERT INTO metrics VALUES (?, {})", marks.trim_end_matches(", ")))?;
    let mut params: Vec<&dyn rusqlite::ToSql> = vec![&ts];
    params.extend(row.iter().map(|v| v as &dyn rusqlite::ToSql));
    stmt.execute(params.as_slice()).map(|_| ())
}

fn camel(c: &str) -> String {
    let mut out = String::new();
    let mut up = false;
    for ch in c.chars() {
        if ch == '_' {
            up = true;
        } else if up {
            out.extend(ch.to_uppercase());
            up = false;
        } else {
            out.push(ch);
        }
    }
    out
}

/// Rows averaged per `step` seconds bucket (ts = bucket start), camelCase keys, as GET /api/metrics returns them.
pub fn history(db: &Connection, from: i64, to: i64, step: f64) -> rusqlite::Result<Vec<Map<String, Value>>> {
    let b = (step.floor() as i64).max(1) * 1000;
    let avgs = COLS.iter().map(|c| format!("AVG({c})")).collect::<Vec<_>>().join(", ");
    let mut stmt = db.prepare(&format!(
        "SELECT (CAST(ts AS INTEGER)/{b})*{b}, {avgs} FROM metrics WHERE ts BETWEEN ?1 AND ?2 GROUP BY CAST(ts AS INTEGER)/{b} ORDER BY 1"
    ))?;
    let rows = stmt.query_map([from, to], |r| {
        let mut m = Map::new();
        m.insert("ts".into(), json!(r.get::<_, i64>(0)?));
        for (k, c) in COLS.iter().enumerate() {
            m.insert(camel(c), json!(r.get::<_, Option<f64>>(k + 1)?));
        }
        Ok(m)
    })?;
    rows.collect()
}

/// CPU (% of the whole machine) and RAM (MB) of a part: its process plus its direct children
/// (voice's venv python.exe is only a launcher for the real interpreter).
fn part_usage(sys: &System, pid: u32, ncpu: f32) -> Option<(f64, f64)> {
    let root = Pid::from_u32(pid);
    sys.process(root)?;
    let (mut cpu, mut ram) = (0f32, 0u64);
    for (id, p) in sys.processes() {
        if *id == root || p.parent() == Some(root) {
            cpu += p.cpu_usage();
            ram += p.memory();
        }
    }
    Some((round((cpu / ncpu) as f64), round(ram as f64 / MB)))
}

pub async fn run(sup: Arc<Supervisor>, db: Db, bus: Bus) {
    let nvml = nvml_wrapper::Nvml::init().ok(); // loads nvml.dll from the NVIDIA driver; no GPU numbers without it
    let mut sys = System::new().expect("sysinfo");
    let ncpu = std::thread::available_parallelism().map_or(1, |n| n.get()) as f32;
    let kind = ProcessRefreshKind::nothing().with_cpu().with_memory();
    let mut acc = Acc::default();
    let mut last_row = now_ms();
    let mut rows = 0u32;
    let mut tick = tokio::time::interval(TICK);
    loop {
        tick.tick().await;
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
        let now = now_ms();

        sup.set_external(sys.processes().values().any(|p| p.name().eq_ignore_ascii_case("llama-server.exe")));

        let gpu = nvml.as_ref().and_then(|n| {
            let d = n.device_by_index(0).ok()?;
            let mem = d.memory_info().ok();
            Some(json!({
                "util": d.utilization_rates().ok().map(|u| u.gpu),
                "vramUsed": mem.as_ref().map(|m| round(m.used as f64 / MB)),
                "vramTotal": mem.as_ref().map(|m| round(m.total as f64 / MB)),
                "temp": d.temperature(nvml_wrapper::enum_wrappers::device::TemperatureSensor::Gpu).ok(),
            }))
        });
        let mut parts = Map::new();
        for n in sup.part_names() {
            parts.insert(n, Value::Null);
        }
        for (name, pid) in sup.pids() {
            if let Some((cpu, ram)) = part_usage(&sys, pid, ncpu) {
                parts.insert(name, json!({ "cpu": cpu, "ram": ram }));
            }
        }
        let ram_total = sys.total_memory() as f64 / MB;
        let sample = json!({
            "ts": now,
            "cpu": round(sys.global_cpu_usage() as f64),
            "ram": { "used": round(sys.used_memory() as f64 / MB), "total": round(ram_total) },
            "gpu": gpu,
            "parts": parts,
        });

        let f = |p: &str| sample.pointer(p).and_then(Value::as_f64);
        let row = [
            f("/cpu"), f("/ram/used"), f("/ram/total"), f("/gpu/util"), f("/gpu/vramUsed"), f("/gpu/vramTotal"), f("/gpu/temp"),
            f("/parts/llm/cpu"), f("/parts/llm/ram"), f("/parts/voice/cpu"), f("/parts/voice/ram"), f("/parts/bot/cpu"), f("/parts/bot/ram"),
        ];
        acc.add(&row);
        if now - last_row >= ROW_EVERY_MS {
            let db = db.lock().unwrap();
            let _ = insert(&db, now, &acc.take());
            rows += 1;
            if rows.is_multiple_of(360) {
                let _ = prune(&db); // hourly, so a long-running app keeps 30 days only
            }
            last_row = now;
        }
        let _ = bus.send(Event { name: "metrics", data: sample });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acc_averages_present_values_and_resets() {
        let mut a = Acc::default();
        let mut r = [None; 13];
        r[0] = Some(10.0);
        a.add(&r);
        r[0] = Some(20.0);
        r[3] = Some(5.0);
        a.add(&r);
        let out = a.take();
        assert_eq!(out[0], Some(15.0));
        assert_eq!(out[3], Some(5.0));
        assert_eq!(out[1], None);
        assert_eq!(a.take()[0], None);
    }

    #[test]
    fn history_buckets_rows() {
        let db = Connection::open_in_memory().unwrap();
        init(&db).unwrap();
        let base = now_ms() / 60_000 * 60_000; // minute aligned
        for k in 0..12 {
            let mut r = [None; 13];
            r[0] = Some(k as f64); // cpu = 0..11
            r[7] = (k % 2 == 0).then_some(100.0);
            insert(&db, base + k * 10_000, &r).unwrap();
        }
        // a REAL timestamp, as node:sqlite stored them
        db.execute("INSERT INTO metrics(ts, cpu) VALUES (?1, 50.0)", [(base + 120_000) as f64]).unwrap();

        let h = history(&db, base, base + 119_999, 60.0).unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0]["ts"], base);
        assert_eq!(h[0]["cpu"], 2.5); // avg 0..5
        assert_eq!(h[1]["cpu"], 8.5); // avg 6..11
        assert_eq!(h[0]["llmCpu"], 100.0);
        assert_eq!(h[0]["gpuTemp"], Value::Null);
        assert!(h[0].contains_key("ramUsed") && h[0].contains_key("botRam"));

        let all = history(&db, base, base + 200_000, 0.0).unwrap(); // step 0 -> 1 s buckets
        assert_eq!(all.len(), 13);
        assert_eq!(all[12]["ts"], base + 120_000);
    }

    #[test]
    fn prune_drops_old_rows() {
        let db = Connection::open_in_memory().unwrap();
        init(&db).unwrap();
        insert(&db, now_ms() - KEEP_MS - 1000, &[Some(1.0); 13]).unwrap();
        insert(&db, now_ms(), &[Some(1.0); 13]).unwrap();
        prune(&db).unwrap();
        let n: i64 = db.query_row("SELECT COUNT(*) FROM metrics", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn camel_case_keys() {
        assert_eq!(camel("vram_used"), "vramUsed");
        assert_eq!(camel("cpu"), "cpu");
    }
}
