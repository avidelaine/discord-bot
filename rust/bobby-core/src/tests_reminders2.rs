//! Reminders, review fixes: a failing save never repeats, absent owners get a DM, DST-proof "tomorrow",
//! missed reminders are reported once, one ticker at a time.
use crate::actions::reminders::{self as rem, clock_text, fire_due, next_at_tz, MAX_LATE_MS};
use crate::brain::Brain;
use crate::store::{temp_dir, Store};
use crate::testing::{fake_client, FakeLlm, Sent};
use crate::util::BrainError;
use bobby_proto::msg::{BrainMsg, Cap, Channel, Context, Member};
use chrono::{DateTime, Datelike, Duration, FixedOffset, LocalResult, NaiveDate, NaiveDateTime, Offset, TimeZone, Timelike, Weekday};
use serde_json::{json, Value};

const NOW: i64 = 1_800_000_000_000;

fn item(id: i64, kind: &str, text: &str, due: i64, secs: u64) -> Value {
    json!({ "id": id, "user": "42", "name": "Marie", "kind": kind, "text": text, "due": due, "secs": secs, "lang": "fr" })
}

/// Marie (42) is in the "Général" voice channel or not.
fn ctx(in_voice: bool) -> Context {
    let ch = |n: &str| Channel { id: n.into(), name: n.into() };
    let marie = Member { id: "42".into(), name: "Marie".into(), username: "Marie".into(), global_name: None, channel: in_voice.then(|| "Général".into()), roles: vec![], bot: false };
    Context { speaker: "42".into(), members: vec![marie], voice_channels: vec![ch("Général")], text_channels: vec![ch("dev-bot-test")], bobby_channel: None }
}

/// (said aloud, DMs as (user, text)) so far.
async fn heard(sent: &Sent) -> (Vec<String>, Vec<(String, String)>) {
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let g = sent.lock().unwrap();
    let said = g.iter().filter_map(|m| if let BrainMsg::Say { text, .. } = m { Some(text.clone()) } else { None }).collect();
    let dms = g.iter().filter_map(|m| if let BrainMsg::Run { cap: Cap::Dm { user, text }, .. } = m { Some((user.clone(), text.clone())) } else { None }).collect();
    (said, dms)
}

fn store_with(all: Vec<Value>) -> Store {
    let store = Store::temp();
    store.save("reminders", &Value::Array(all)).unwrap();
    store
}

#[test]
fn a_failing_save_never_repeats_a_reminder() {
    let store = store_with(vec![item(1, "reminder", "sortir le linge", NOW - 1000, 60), item(2, "reminder", "plus tard", NOW + 60_000, 60)]);
    let failing = |_: &Store, _: Vec<Value>| Err(BrainError::failed("disk full"));
    assert_eq!(rem::take_due(&store, NOW, false, failing).len(), 1);
    // the disk still holds it, but it is out of memory: the next ticks do not return it
    for t in 1..4 {
        assert!(rem::take_due(&store, NOW + t * 1000, false, failing).is_empty());
    }
    assert_eq!(store.load_or("reminders", json!([])).as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn a_reminder_for_someone_in_voice_is_said_aloud() {
    let store = store_with(vec![item(1, "reminder", "sortir le linge", NOW - 1000, 60)]);
    let (io, sent) = fake_client(|_| Ok(Value::Null));
    assert_eq!(fire_due(&store, &io, NOW, Some(&ctx(true))).await, 1);
    let (said, dms) = heard(&sent).await;
    assert_eq!(said, ["Marie, rappel : sortir le linge."]);
    assert!(dms.is_empty());
}

#[tokio::test]
async fn a_reminder_for_someone_absent_is_a_dm_never_said_aloud() {
    let store = store_with(vec![item(1, "reminder", "sortir le linge", NOW - 1000, 60), item(2, "timer", "", NOW - 1000, 300)]);
    let (io, sent) = fake_client(|_| Ok(Value::Null));
    assert_eq!(fire_due(&store, &io, NOW, Some(&ctx(false))).await, 2);
    let (said, dms) = heard(&sent).await;
    assert!(said.is_empty(), "said aloud: {said:?}");
    assert_eq!(dms, [("42".to_string(), "Rappel : sortir le linge".to_string()), ("42".to_string(), "Ton minuteur de 5 minutes est fini.".to_string())]);
    assert!(store.load_or("reminders", json!([])).as_array().unwrap().is_empty());
}

#[tokio::test]
async fn without_a_snapshot_a_fresh_reminder_waits() {
    let store = store_with(vec![item(1, "reminder", "sortir le linge", NOW - 1000, 60)]);
    let (io, sent) = fake_client(|_| Ok(Value::Null));
    assert_eq!(fire_due(&store, &io, NOW, None).await, 0);
    let (said, dms) = heard(&sent).await;
    assert!(said.is_empty() && dms.is_empty());
    assert_eq!(fire_due(&store, &io, NOW + 1000, Some(&ctx(true))).await, 1); // the snapshot arrived
}

#[tokio::test]
async fn a_missed_reminder_is_reported_once_by_dm() {
    let due = NOW - MAX_LATE_MS - 1;
    let store = store_with(vec![item(1, "reminder", "vieux rappel", due, 60), item(2, "timer", "", due, 120)]);
    let (io, sent) = fake_client(|_| Ok(Value::Null));
    assert_eq!(fire_due(&store, &io, NOW, Some(&ctx(true))).await, 0);
    assert_eq!(fire_due(&store, &io, NOW + 1000, Some(&ctx(true))).await, 0); // once
    let (said, dms) = heard(&sent).await;
    assert!(said.is_empty());
    let at = clock_text(due, "fr");
    assert_eq!(
        dms,
        [
            ("42".to_string(), format!("Pendant que j'étais éteint, j'ai raté ton rappel : vieux rappel ({at}).")),
            ("42".to_string(), format!("Pendant que j'étais éteint, j'ai raté ton rappel : minuteur de 2 minutes ({at}).")),
        ]
    );
}

// ---------- DST ----------

/// Europe/Paris rules (CET +1, CEST +2 from the last Sunday of March to the last Sunday of October, 01:00 UTC).
#[derive(Clone, Copy, Debug)]
struct Paris;
#[derive(Clone, Copy, Debug, PartialEq)]
struct Off(i32);
impl Offset for Off {
    fn fix(&self) -> FixedOffset {
        FixedOffset::east_opt(self.0).unwrap()
    }
}
impl std::fmt::Display for Off {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:+}h", self.0 / 3600)
    }
}

fn last_sunday(y: i32, m: u32) -> NaiveDate {
    let mut d = NaiveDate::from_ymd_opt(y, m, 31).unwrap();
    while d.weekday() != Weekday::Sun {
        d = d.pred_opt().unwrap();
    }
    d
}

fn off_at_utc(u: &NaiveDateTime) -> Off {
    let (start, end) = (last_sunday(u.year(), 3).and_hms_opt(1, 0, 0).unwrap(), last_sunday(u.year(), 10).and_hms_opt(1, 0, 0).unwrap());
    Off(if *u >= start && *u < end { 7200 } else { 3600 })
}

impl TimeZone for Paris {
    type Offset = Off;
    fn from_offset(_: &Off) -> Paris {
        Paris
    }
    fn offset_from_local_date(&self, d: &NaiveDate) -> LocalResult<Off> {
        self.offset_from_local_datetime(&d.and_hms_opt(12, 0, 0).unwrap())
    }
    fn offset_from_local_datetime(&self, l: &NaiveDateTime) -> LocalResult<Off> {
        let ok: Vec<Off> = [7200, 3600].into_iter().map(Off).filter(|o| off_at_utc(&(*l - Duration::seconds(o.0 as i64))) == *o).collect();
        match ok[..] {
            [] => LocalResult::None,
            [o] => LocalResult::Single(o),
            [a, b] => LocalResult::Ambiguous(a, b),
            _ => unreachable!(),
        }
    }
    fn offset_from_utc_date(&self, d: &NaiveDate) -> Off {
        self.offset_from_utc_datetime(&d.and_hms_opt(12, 0, 0).unwrap())
    }
    fn offset_from_utc_datetime(&self, u: &NaiveDateTime) -> Off {
        off_at_utc(u)
    }
}

fn paris(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Paris> {
    Paris.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
}

#[test]
fn tomorrow_keeps_its_wall_clock_hour_across_dst() {
    // spring forward: 2026-03-29 has 23 hours; 8h that morning is 06:00 UTC (CEST), +24 h would give 9h
    let r = next_at_tz(paris(2026, 3, 28, 20, 0), 8, 0).unwrap();
    assert_eq!((r.day(), r.hour(), r.naive_utc().hour()), (29, 8, 6));
    // fall back: 2026-10-25 has 25 hours; 8h is 07:00 UTC (CET), +24 h would give 7h
    let r = next_at_tz(paris(2026, 10, 24, 20, 0), 8, 0).unwrap();
    assert_eq!((r.day(), r.hour(), r.naive_utc().hour()), (25, 8, 7));
    // a normal day, and "later today" still works on a DST day
    assert_eq!(next_at_tz(paris(2026, 11, 20, 18, 0), 8, 0).unwrap(), paris(2026, 11, 21, 8, 0));
    assert_eq!(next_at_tz(paris(2026, 3, 29, 0, 30), 8, 0).unwrap(), paris(2026, 3, 29, 8, 0));
    // 02:30 on 2026-03-29 does not exist
    assert!(next_at_tz(paris(2026, 3, 28, 20, 0), 2, 30).is_none());
}

// ---------- One ticker ----------

#[tokio::test]
async fn two_tickers_cannot_coexist() {
    let (io, _sent) = fake_client(|_| Ok(Value::Null));
    let brain = std::sync::Arc::new(Brain::new(Store::temp(), FakeLlm::default(), io, temp_dir("ticker").join("conversations.jsonl")));
    let first = rem::spawn_ticker(brain.clone()).expect("the first ticker starts");
    assert!(rem::spawn_ticker(brain.clone()).is_none(), "a second ticker must not start");
    first.abort();
    let _ = first.await; // the task is dropped, its claim released
    let again = rem::spawn_ticker(brain).expect("a new run can start its own ticker");
    again.abort();
}
