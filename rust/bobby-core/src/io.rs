//! The brain's only way out: `BrainMsg`s to a client, and `ClientMsg::Result`s back for `Run`.
//! Replaces Node's `io` object (Discord refs): `io.say` -> `Say`, `io.beep/nope/ack` -> `Signal`,
//! `io.hush` -> `Hush`, `io.music.*` / `io.post` / `io.memberName` / voice moves -> `Run { cap }`.
use crate::util::{BrainError, Res};
use bobby_proto::msg::{BrainMsg, Cap, SignalKind, Voice};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use tokio::sync::{mpsc, oneshot};

/// Discord's "Missing Permissions" error code, sent by the client as `error: "50013 ..."`.
pub const NO_RIGHTS: &str = "50013";

/// How long a `run` waits for the client's answer.
const RUN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

type Reply = Result<Value, String>;

pub struct Io {
    tx: mpsc::UnboundedSender<BrainMsg>,
    waiting: Mutex<HashMap<u64, oneshot::Sender<Reply>>>,
    next: AtomicU64,
}

impl Io {
    /// The brain side and the receiver the client (or a test) reads.
    pub fn new() -> (Io, mpsc::UnboundedReceiver<BrainMsg>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Io { tx, waiting: Mutex::new(HashMap::new()), next: AtomicU64::new(1) }, rx)
    }

    pub fn send(&self, m: BrainMsg) {
        let _ = self.tx.send(m); // no client: nothing to do (Node's optional io.x?.())
    }

    /// Asks the client to do `cap` and waits for its `Result`. An error starting with "50013" means Discord refused.
    pub async fn run(&self, cap: Cap) -> Res<Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.waiting().insert(id, tx);
        if self.tx.send(BrainMsg::Run { id, cap }).is_err() {
            self.waiting().remove(&id);
            return Err(BrainError::failed("no client"));
        }
        match tokio::time::timeout(RUN_TIMEOUT, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(BrainError::Failed { no_rights: e.starts_with(NO_RIGHTS), msg: e }),
            Ok(Err(_)) => Err(BrainError::failed("client gone")),
            Err(_) => {
                self.waiting().remove(&id);
                Err(BrainError::failed("client timeout"))
            }
        }
    }

    fn waiting(&self) -> std::sync::MutexGuard<'_, HashMap<u64, oneshot::Sender<Reply>>> {
        self.waiting.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Routes a `ClientMsg::Result` to the `run` waiting for it. False if nobody waits for that id.
    pub fn deliver(&self, id: u64, ok: bool, data: Value, error: Option<String>) -> bool {
        let Some(tx) = self.waiting().remove(&id) else { return false };
        let _ = tx.send(if ok { Ok(data) } else { Err(error.unwrap_or_else(|| "failed".into())) });
        true
    }

    /// `io.say(text, lang, voice, userId)`; empty text says nothing.
    pub fn say(&self, text: &str, lang: &str, voice: Voice, to_user: Option<&str>) {
        if !text.is_empty() {
            self.send(BrainMsg::Say { text: text.into(), lang: lang.into(), voice, to_user: to_user.map(Into::into) });
        }
    }

    pub fn signal(&self, kind: SignalKind) {
        self.send(BrainMsg::Signal { kind });
    }

    pub fn hush(&self) {
        self.send(BrainMsg::Hush);
    }

    pub fn expect(&self, user: &str, ms: u32) {
        self.send(BrainMsg::Expect { user: user.into(), ms });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn run_waits_for_its_result() {
        let (io, mut rx) = Io::new();
        let io = std::sync::Arc::new(io);
        let client = {
            let io = io.clone();
            tokio::spawn(async move {
                while let Some(m) = rx.recv().await {
                    if let BrainMsg::Run { id, cap } = m {
                        match cap {
                            Cap::MemberName { .. } => io.deliver(id, true, json!({"name": "Kévin"}), None),
                            _ => io.deliver(id, false, Value::Null, Some("50013 Missing Permissions".into())),
                        };
                    }
                }
            })
        };
        assert_eq!(io.run(Cap::MemberName { id: "1".into() }).await.unwrap(), json!({"name": "Kévin"}));
        assert!(matches!(io.run(Cap::Leave).await, Err(BrainError::Failed { no_rights: true, .. })));
        client.abort();
    }

    #[tokio::test]
    async fn run_fails_without_a_client() {
        let (io, rx) = Io::new();
        drop(rx);
        assert!(matches!(io.run(Cap::Leave).await, Err(BrainError::Failed { msg, .. }) if msg == "no client"));
        assert!(io.waiting().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn run_times_out() {
        let (io, _rx) = Io::new(); // a client that never answers
        assert!(matches!(io.run(Cap::Leave).await, Err(BrainError::Failed { msg, .. }) if msg == "client timeout"));
        assert!(io.waiting().is_empty());
    }
}
