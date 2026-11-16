// The journal (_logs/conversations.jsonl) read from the end, its live new lines, and log file tails. Port of app/server.js.
use crate::supervisor::{Bus, Event};
use serde_json::{json, Map, Value};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

const CHUNK: u64 = 256 * 1024;

fn read_at(f: &mut File, pos: u64, len: u64) -> std::io::Result<Vec<u8>> {
    let mut buf = vec![0; len as usize];
    f.seek(SeekFrom::Start(pos))?;
    f.read_exact(&mut buf)?;
    Ok(buf)
}

/// Newest first, at most `limit` items, only those with `at` < `before`, stopping at the first one older than `from`.
/// ponytail: assumes the file is in chronological order (`at` is an ISO string, compared as text like the Node app).
pub fn read(path: &Path, limit: usize, before: Option<&str>, from: Option<&str>) -> Vec<Value> {
    let mut items = Vec::new();
    let Ok(mut f) = File::open(path) else { return items };
    let mut pos = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut rest: Vec<u8> = Vec::new(); // start of the line cut by the previous (later) chunk
    while pos > 0 {
        let len = CHUNK.min(pos);
        pos -= len;
        let Ok(mut buf) = read_at(&mut f, pos, len) else { break };
        buf.extend_from_slice(&rest);
        let start = if pos > 0 {
            match buf.iter().position(|&b| b == b'\n') {
                Some(i) => i + 1,
                None => {
                    rest = buf; // a line longer than a chunk: keep growing it
                    continue;
                }
            }
        } else {
            0
        };
        for line in buf[start..].split(|&b| b == b'\n').rev() {
            let Ok(o) = serde_json::from_slice::<Value>(line.trim_ascii()) else { continue };
            let at = o.get("at").and_then(Value::as_str).unwrap_or("");
            if from.is_some_and(|f| at < f) {
                return items;
            }
            if before.is_some_and(|b| at >= b) {
                continue;
            }
            items.push(o);
            if items.len() >= limit {
                return items;
            }
        }
        rest = buf[..start.saturating_sub(1)].to_vec();
    }
    items
}

pub fn stats(items: &[Value]) -> Value {
    let mut by_outcome = Map::new();
    let mut totals: Vec<f64> = Vec::new();
    for it in items {
        let key = match it.get("outcome") {
            Some(Value::String(s)) => s.clone(),
            Some(v) if !v.is_null() => v.to_string(),
            _ => "undefined".into(), // JS object key of a missing outcome
        };
        let n = by_outcome.get(&key).and_then(Value::as_u64).unwrap_or(0);
        by_outcome.insert(key, json!(n + 1));
        if let Some(t) = it.pointer("/timing/total").and_then(Value::as_f64) {
            totals.push(t);
        }
    }
    totals.sort_by(f64::total_cmp);
    let m = totals.len() / 2;
    let median = match totals.len() {
        0 => Value::Null,
        n if n % 2 == 1 => json!(totals[m]),
        _ => json!((totals[m - 1] + totals[m]) / 2.0),
    };
    json!({ "count": items.len(), "byOutcome": by_outcome, "medianTotalMs": median })
}

/// Normalizes a date query parameter to the ISO form the journal uses (`2026-11-16T08:00:00.000Z`), or None.
pub fn iso_or_null(v: Option<&str>) -> Option<String> {
    let d = chrono::DateTime::parse_from_rfc3339(v?.trim()).ok()?;
    Some(d.with_timezone(&chrono::Utc).format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
}

/// The last `n` lines of a text file (reads only the end).
pub fn tail_lines(path: &Path, n: usize) -> Vec<String> {
    let Ok(mut f) = File::open(path) else { return vec![] };
    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
    let mut len = 64 * 1024u64;
    let text = loop {
        len = len.min(size);
        let Ok(buf) = read_at(&mut f, size - len, len) else { return vec![] };
        let text = String::from_utf8_lossy(&buf).into_owned();
        if len >= size || text.matches('\n').count() > n {
            break text;
        }
        len *= 4;
    };
    let mut lines: Vec<String> = text.split('\n').map(|l| l.trim_end_matches('\r').to_string()).collect();
    if len < size {
        lines.remove(0); // first line is cut
    }
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let skip = lines.len().saturating_sub(n);
    lines.split_off(skip)
}

/// Follows the journal: polls its size every second and publishes each new complete line as a `journal` event.
pub async fn follow(path: PathBuf, bus: Bus) {
    let mut pos = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let mut rest: Vec<u8> = Vec::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        tick.tick().await;
        let Ok(size) = std::fs::metadata(&path).map(|m| m.len()) else { continue };
        if size < pos {
            // truncated
            pos = size;
            rest.clear();
            continue;
        }
        if size == pos {
            continue;
        }
        let Ok(buf) = File::open(&path).and_then(|mut f| read_at(&mut f, pos, size - pos)) else { continue };
        pos = size;
        rest.extend_from_slice(&buf);
        let Some(nl) = rest.iter().rposition(|&b| b == b'\n') else { continue };
        for line in rest[..nl].split(|&b| b == b'\n') {
            if let Ok(o) = serde_json::from_slice::<Value>(line.trim_ascii()) {
                let _ = bus.send(Event { name: "journal", data: o });
            }
        }
        rest.drain(..=nl); // a partial last line waits for its end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("bobby-journal-{}-{name}", std::process::id()))
    }

    fn write_journal(path: &Path, n: usize, pad: usize) {
        let mut f = File::create(path).unwrap();
        for i in 0..n {
            let at = format!("2026-11-16T10:{:02}:{:02}.000Z", i / 60, i % 60);
            let outcome = if i % 3 == 0 { "executed" } else { "ignored" };
            writeln!(f, "{}", json!({ "at": at, "i": i, "outcome": outcome, "timing": { "total": i }, "pad": "x".repeat(pad) })).unwrap();
            if i == 5 {
                writeln!(f, "not json").unwrap();
            }
        }
    }

    #[test]
    fn reads_newest_first_across_chunks() {
        let p = tmp("chunks");
        write_journal(&p, 1000, 1000); // ~1 MB: several 256 KB chunks, lines cut at chunk edges
        let all = read(&p, 5000, None, None);
        assert_eq!(all.len(), 1000);
        assert!(all.iter().enumerate().all(|(k, o)| o["i"] == 999 - k as u64));
        let top = read(&p, 10, None, None);
        assert_eq!(top.len(), 10);
        assert_eq!(top[0]["i"], 999);
        let page = read(&p, 3, Some("2026-11-16T10:01:00.000Z"), None);
        assert_eq!(page.iter().map(|o| o["i"].as_u64().unwrap()).collect::<Vec<_>>(), [59, 58, 57]);
        let since = read(&p, 5000, None, Some("2026-11-16T10:16:00.000Z"));
        assert_eq!(since.len(), 40); // 960..999
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn handles_missing_and_long_lines() {
        assert!(read(Path::new("Z:/nope/none.jsonl"), 10, None, None).is_empty());
        let p = tmp("long");
        write_journal(&p, 3, 600_000); // each line longer than a chunk
        let all = read(&p, 10, None, None);
        assert_eq!(all.iter().map(|o| o["i"].as_u64().unwrap()).collect::<Vec<_>>(), [2, 1, 0]);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn stats_count_outcomes_and_median() {
        let items: Vec<Value> = (1..=4).map(|t| json!({ "outcome": if t == 1 { "failed" } else { "executed" }, "timing": { "total": t * 100 } })).collect();
        let s = stats(&items);
        assert_eq!(s["count"], 4);
        assert_eq!(s["byOutcome"]["executed"], 3);
        assert_eq!(s["medianTotalMs"], 250.0);
        assert_eq!(stats(&items[..3])["medianTotalMs"], 200.0);
        assert_eq!(stats(&[])["medianTotalMs"], Value::Null);
    }

    #[test]
    fn iso_normalization() {
        assert_eq!(iso_or_null(Some("2026-11-16T09:00:00+01:00")).as_deref(), Some("2026-11-16T08:00:00.000Z"));
        assert_eq!(iso_or_null(Some("2026-11-15T23:00:00.000Z")).as_deref(), Some("2026-11-15T23:00:00.000Z"));
        assert_eq!(iso_or_null(Some("garbage")), None);
        assert_eq!(iso_or_null(None), None);
    }

    #[test]
    fn tail_reads_last_lines() {
        let p = tmp("tail");
        let mut f = File::create(&p).unwrap();
        for i in 0..20000 {
            write!(f, "line {i}\r\n").unwrap();
        }
        drop(f);
        let t = tail_lines(&p, 3);
        assert_eq!(t, ["line 19997", "line 19998", "line 19999"]);
        assert_eq!(tail_lines(&p, 15000).len(), 15000);
        assert_eq!(tail_lines(&p, 15000)[0], "line 5000");
        assert!(tail_lines(Path::new("Z:/none.log"), 3).is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[tokio::test]
    async fn follow_publishes_complete_new_lines() {
        let p = tmp("follow");
        std::fs::write(&p, "{\"at\":\"old\"}\n").unwrap();
        let (bus, mut rx) = tokio::sync::broadcast::channel(16);
        tokio::spawn(follow(p.clone(), bus));
        tokio::time::sleep(Duration::from_millis(300)).await;
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        write!(f, "{{\"at\":\"new\"}}\n{{\"at\":\"part").unwrap();
        let e = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap();
        assert_eq!(e.data["at"], "new");
        writeln!(f, "ial\"}}").unwrap();
        let e = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await.unwrap().unwrap();
        assert_eq!(e.data["at"], "partial");
        let _ = std::fs::remove_file(&p);
    }
}
