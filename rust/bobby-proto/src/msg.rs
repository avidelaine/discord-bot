//! Messages between the brain and its clients (Discord client, PC audio client, later a phone).
//!
//! Today they travel over an in-process channel; later the same values are JSON over a WebSocket.
//! Every message is internally tagged (`"type"`), every capability request too (`"cap"`).
//! Ids are Discord snowflakes as strings (never names, never numbers: JSON numbers lose precision).
//! Audio is NOT a JSON message: PCM travels as binary frames (or a separate channel) tagged with the
//! utterance id; `ClientMsg::Utterance` only marks where an utterance starts and ends.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Sent in `Hello`; bump on any breaking change of the wire shape.
pub const PROTO_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Discord,
    Pc,
    Phone,
}

/// One variant per family of `Cap`, announced in `Hello` so the brain knows what a client can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapKind {
    DiscordVoice,
    DiscordText,
    Music,
    PcMedia,
    PcVolume,
    PcApps,
    PcPower,
    /// Every client that speaks (Discord or PC).
    Voice,
}

// ---------- Context ----------

/// What `context(member, guild)` of the old brain builds: the server as plain data. Resent on change.
/// Nicknames are not here: the brain stores them.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Context {
    /// Id of the person speaking.
    pub speaker: String,
    pub members: Vec<Member>,
    pub voice_channels: Vec<Channel>,
    pub text_channels: Vec<Channel>,
    /// Text channel where Bobby posts by default.
    pub bobby_channel: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Member {
    pub id: String,
    /// Display name in the server.
    pub name: String,
    pub username: String,
    pub global_name: Option<String>,
    /// Voice channel id, if connected.
    pub channel: Option<String>,
    /// Role names and ids (the "Team Prime" check looks at both).
    pub roles: Vec<String>,
    pub bot: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: String,
}

// ---------- Client -> brain ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Hello { v: u32, device: String, kind: ClientKind, caps: Vec<CapKind> },
    Context(Context),
    /// Wake word heard mid-sentence.
    Wake { utterance: u64, user: Option<String>, score: f32 },
    /// Start (`end: false`) or end (`end: true`) of an utterance whose PCM goes as binary frames.
    Utterance { utterance: u64, user: String, end: bool },
    /// A client that transcribed by itself (today: STT is a service called by address).
    Heard {
        utterance: u64,
        user: String,
        text: String,
        lang: String,
        score: Option<f32>,
        stt_ms: u32,
        /// The brain was listening without a wake word.
        expected: bool,
    },
    /// Answer to `BrainMsg::Run { id }`. `data` is the plain-data result (see `Cap` docs).
    /// `error` starting with "50013" means Discord refused for missing permissions (said as such to the user).
    Result { id: u64, ok: bool, data: Value, error: Option<String> },
    /// A slash command (`join`, `leave`, `surnoms`) typed by `user` in `text_channel`. The brain decides
    /// (roles, nicknames are its data) and answers with `BrainMsg::Reply { id }`, then any `Run`.
    Command { id: u64, name: String, user: String, text_channel: String },
    /// Bobby's voice started (`user`: the person the reply is for, see `Say::to_user`) or ended (`None`).
    /// The brain needs it to let that person interrupt with a plain "stop".
    Speaking { user: Option<String> },
    /// A direct message someone wrote to Bobby. `reply_to`: the id of Bobby's message it answers (Discord "reply"),
    /// which is how the messenger tells an answer from an unrelated DM.
    Dm { user: String, text: String, reply_to: Option<String> },
}

// ---------- Brain -> client ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    /// Spoken or short "got it": the client plays its spoken ack, or the beep if it has none yet.
    Ack,
    Beep,
    Nope,
    /// The speaking ring: Bobby is listening.
    Listening,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Voice {
    Bobby,
    Claude,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum BrainMsg {
    Signal { kind: SignalKind },
    /// The client synthesizes through the TTS service. Interruptible by `Hush`, or by a "stop" from `to_user`.
    Say { text: String, lang: String, voice: Voice, to_user: Option<String> },
    /// Stop speaking now.
    Hush,
    Run { id: u64, cap: Cap },
    /// Listen to `user` without a wake word for `ms` milliseconds. `ms: 0` closes the window.
    Expect { user: String, ms: u32 },
    /// Answer to `ClientMsg::Command { id }`. `ephemeral`: only the person who typed it sees it.
    Reply { id: u64, text: Option<String>, embeds: Vec<Embed>, ephemeral: bool },
}

// ---------- Capabilities ----------

/// Plain Discord embed (one logical card); the client splits it to Discord's limits like `bot/format.js` `embed()`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Embed {
    pub title: String,
    pub description: String,
    pub color: Option<u32>,
    #[serde(default)]
    pub fields: Vec<EmbedField>,
    #[serde(default)]
    pub footer: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EmbedField {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub inline: bool,
}

/// What to play. `endless`: go on with similar tracks (`similar`) or only this artist (`artist`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MusicItem {
    Radio { style: String },
    Style { style: String },
    Youtube { query: String, endless: Option<Endless>, seed: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Endless {
    Similar,
    Artist,
}

/// Volume change, 0..=100. Music accepts `Set` and `Delta`; the PC accepts all of them.
/// Replies carry `{"level": n, "muted": bool}` (PC) or `{"volume": n}` (music).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum VolumeOp {
    Get,
    Set { level: u8 },
    Delta { delta: i8 },
    Mute,
    Unmute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaCmd {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
}

/// Every side effect the brain can ask a client to do. The client answers with `ClientMsg::Result`.
///
/// Result `data` per cap (everything else answers `null`):
/// - `mute`/`deafen`: `{"prev": {user_id: bool}}` (state before the change; the brain undoes it by
///   sending the old values back, one call per value)
/// - `move`: `{"prev": {user_id: channel_id|null}}`
/// - `join`: `{"prev": channel_id|null}` (where Bobby was; null if it did not move), used to undo a "come"/"move me"
/// - `post`: `{"channel": name}`; `edit_last`/`delete_last`: `{"found": bool}`
/// - `clean`: `{"count": n, "channel": name}` (deletes in the background, answers at once)
/// - `member_name`: `{"name": string}`
/// - `music_*`: the player's plain result (`{"nowPlaying": ..}`, `{"queue": [..]}`, `{"error": ".."}` ...). The player's own
///   errors arrive as `ok: true` data `{"error": ".."}`, including `"no player"` when the client has no Discord player;
///   `ok: false` is a real failure of the cap.
/// - `pc_now`: `{"playing": bool, "title": .., "artist": ..}`; `pc_volume`: see `VolumeOp`;
///   `pc_apps`/`pc_windows`: `{"items": [string]}`; `pc_launch`/`pc_close`: `{"found": name|null}`;
///   `pc_gpu`: `{"name", "temp", "load", "used", "total"}`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cap", rename_all = "snake_case")]
pub enum Cap {
    // Discord voice moderation
    Mute { users: Vec<String>, on: bool },
    Deafen { users: Vec<String>, on: bool },
    Move { users: Vec<String>, channel: String },
    Disconnect { users: Vec<String> },
    // Bobby itself. `Join` also covers "follow the speaker"; `Leave` stops the music, then disconnects.
    Join { channel: String },
    Leave,
    // Text. `channel: None` is Bobby's default text channel.
    Post { channel: Option<String>, text: Option<String>, embeds: Vec<Embed> },
    /// Mention users only, never @everyone / @here.
    Ping { users: Vec<String>, channel: Option<String>, text: String },
    EditLast { channel: Option<String>, text: String },
    DeleteLast { channel: Option<String> },
    /// Delete all Bobby's own messages of the channel.
    Clean { channel: Option<String> },
    MemberName { id: String },
    /// Direct message to one member (the messenger when they are not in voice). Their written answers come back
    /// as `ClientMsg::Dm`. Answers `{"message_id": "<id>"}`, the id a reply will carry in `reply_to`.
    Dm { user: String, text: String },
    // Music (Bobby's own player, Discord only)
    /// `add: false` plays now, `add: true` queues (starts if nothing plays).
    MusicPlay { item: MusicItem, add: bool },
    MusicStop,
    MusicSkip,
    MusicReplay,
    MusicPause,
    MusicResume,
    MusicVolume { op: VolumeOp },
    MusicNow,
    MusicQueue,
    // PC
    PcMedia { cmd: MediaCmd },
    PcNow,
    PcVolume { op: VolumeOp },
    /// Volume of Bobby's own voice on the output he speaks on (Discord or PC), not the music, not Windows.
    /// The client changes its own `[audio] voice_volume_discord` or `voice_volume_pc` and saves the settings file.
    /// Accepts `Set` and `Delta`; answers `{"level": u8}`.
    VoiceVolume { op: VolumeOp },
    PcApps,
    PcWindows,
    PcLaunch { name: String },
    PcClose { name: String },
    PcLock,
    PcSleep,
    PcShutdown,
    PcGpu,
}

impl Cap {
    pub fn kind(&self) -> CapKind {
        use Cap::*;
        match self {
            Mute { .. } | Deafen { .. } | Move { .. } | Disconnect { .. } | Join { .. } | Leave => CapKind::DiscordVoice,
            Post { .. } | Ping { .. } | EditLast { .. } | DeleteLast { .. } | Clean { .. } | MemberName { .. } | Dm { .. } => CapKind::DiscordText,
            MusicPlay { .. } | MusicStop | MusicSkip | MusicReplay | MusicPause | MusicResume | MusicVolume { .. } | MusicNow | MusicQueue => CapKind::Music,
            PcMedia { .. } | PcNow => CapKind::PcMedia,
            PcVolume { .. } => CapKind::PcVolume,
            PcApps | PcWindows | PcLaunch { .. } | PcClose { .. } => CapKind::PcApps,
            PcLock | PcSleep | PcShutdown | PcGpu => CapKind::PcPower,
            VoiceVolume { .. } => CapKind::Voice,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ids(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    fn all_caps() -> Vec<Cap> {
        let s = |x: &str| x.to_string();
        vec![
            Cap::Mute { users: ids(&["1"]), on: true },
            Cap::Deafen { users: ids(&["1", "2"]), on: false },
            Cap::Move { users: ids(&["1"]), channel: s("2") },
            Cap::Disconnect { users: ids(&["1"]) },
            Cap::Join { channel: s("2") },
            Cap::Leave,
            Cap::Post { channel: None, text: Some(s("hi")), embeds: vec![Embed { title: s("t"), description: s("d"), color: Some(255), fields: vec![EmbedField { name: s("n"), value: s("v"), inline: true }], footer: Some(s("f")) }] },
            Cap::Ping { users: ids(&["1"]), channel: Some(s("3")), text: s("wake up") },
            Cap::EditLast { channel: None, text: s("new") },
            Cap::DeleteLast { channel: Some(s("3")) },
            Cap::Clean { channel: None },
            Cap::MemberName { id: s("1") },
            Cap::MusicPlay { item: MusicItem::Youtube { query: s("daft punk"), endless: Some(Endless::Similar), seed: None }, add: false },
            Cap::MusicPlay { item: MusicItem::Radio { style: s("jazz") }, add: true },
            Cap::MusicPlay { item: MusicItem::Style { style: s("lofi") }, add: true },
            Cap::MusicStop,
            Cap::MusicSkip,
            Cap::MusicReplay,
            Cap::MusicPause,
            Cap::MusicResume,
            Cap::MusicVolume { op: VolumeOp::Set { level: 40 } },
            Cap::MusicNow,
            Cap::MusicQueue,
            Cap::PcMedia { cmd: MediaCmd::Next },
            Cap::PcNow,
            Cap::PcVolume { op: VolumeOp::Delta { delta: -10 } },
            Cap::PcVolume { op: VolumeOp::Mute },
            Cap::VoiceVolume { op: VolumeOp::Delta { delta: 10 } },
            Cap::VoiceVolume { op: VolumeOp::Set { level: 50 } },
            Cap::PcApps,
            Cap::PcWindows,
            Cap::PcLaunch { name: s("Valorant") },
            Cap::PcClose { name: s("Apex") },
            Cap::PcLock,
            Cap::PcSleep,
            Cap::PcShutdown,
            Cap::PcGpu,
        ]
    }

    fn round_trip<T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug>(v: &T) {
        let json = serde_json::to_string(v).unwrap();
        let back: T = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, v, "round trip of {json}");
    }

    #[test]
    fn every_cap_round_trips() {
        for cap in all_caps() {
            round_trip(&cap);
            round_trip(&BrainMsg::Run { id: 1, cap });
        }
    }

    #[test]
    fn every_cap_family_is_covered() {
        let kinds: Vec<CapKind> = all_caps().iter().map(Cap::kind).collect();
        for k in [CapKind::DiscordVoice, CapKind::DiscordText, CapKind::Music, CapKind::PcMedia, CapKind::PcVolume, CapKind::PcApps, CapKind::PcPower, CapKind::Voice] {
            assert!(kinds.contains(&k), "{k:?}");
        }
    }

    #[test]
    fn every_message_round_trips() {
        let s = |x: &str| x.to_string();
        let ctx = Context {
            speaker: s("1"),
            members: vec![Member {
                id: s("1"),
                name: s("Alex"),
                username: s("alex"),
                global_name: Some(s("Alex G")),
                channel: Some(s("2")),
                roles: ids(&["Team Prime", "99"]),
                bot: false,
            }],
            voice_channels: vec![Channel { id: s("2"), name: s("General") }],
            text_channels: vec![Channel { id: s("3"), name: s("bobby") }],
            bobby_channel: Some(s("3")),
        };
        let client = vec![
            ClientMsg::Hello { v: PROTO_VERSION, device: s("pc-alex"), kind: ClientKind::Pc, caps: vec![CapKind::PcMedia, CapKind::PcVolume] },
            ClientMsg::Context(ctx),
            ClientMsg::Wake { utterance: 5, user: None, score: 0.5 },
            ClientMsg::Utterance { utterance: 5, user: s("1"), end: true },
            ClientMsg::Heard { utterance: 5, user: s("1"), text: s("coupe le micro"), lang: s("fr"), score: Some(0.75), stt_ms: 120, expected: false },
            ClientMsg::Result { id: 7, ok: true, data: json!({"prev": {"1": false}}), error: None },
            ClientMsg::Result { id: 8, ok: false, data: Value::Null, error: Some(s("no player")) },
            ClientMsg::Command { id: 9, name: s("join"), user: s("1"), text_channel: s("3") },
            ClientMsg::Speaking { user: Some(s("1")) },
        ];
        for m in &client {
            round_trip(m);
        }
        let brain = vec![
            BrainMsg::Signal { kind: SignalKind::Ack },
            BrainMsg::Signal { kind: SignalKind::Beep },
            BrainMsg::Signal { kind: SignalKind::Nope },
            BrainMsg::Signal { kind: SignalKind::Listening },
            BrainMsg::Say { text: s("Salut"), lang: s("fr"), voice: Voice::Claude, to_user: Some(s("1")) },
            BrainMsg::Hush,
            BrainMsg::Expect { user: s("1"), ms: 8000 },
            BrainMsg::Reply { id: 9, text: Some(s("À plus.")), embeds: vec![], ephemeral: true },
        ];
        for m in &brain {
            round_trip(m);
        }
    }

    #[test]
    fn run_wire_shape_is_pinned() {
        let msg = BrainMsg::Run { id: 7, cap: Cap::Move { users: ids(&["1"]), channel: "2".into() } };
        assert_eq!(serde_json::to_string(&msg).unwrap(), r#"{"type":"run","id":7,"cap":{"cap":"move","users":["1"],"channel":"2"}}"#);
        let vol = BrainMsg::Run { id: 8, cap: Cap::PcVolume { op: VolumeOp::Delta { delta: -10 } } };
        assert_eq!(serde_json::to_string(&vol).unwrap(), r#"{"type":"run","id":8,"cap":{"cap":"pc_volume","op":{"op":"delta","delta":-10}}}"#);
    }
}
