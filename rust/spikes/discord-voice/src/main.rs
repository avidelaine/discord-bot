//! R0 spike: can Bobby's Discord voice layer live in Rust (songbird 0.6 + DAVE)?
//! Joins the voice channel of one user, saves each sentence (600 ms of silence ends it) as
//! 48 kHz stereo s16le WAV, twice: decoded by songbird (VoiceTick) and decoded by us per packet
//! (RtpPacket, like bot/bot.js). Beeps on join and after each sentence. Ctrl+C prints totals.

use serenity::all::{ChannelId, Context, EventHandler, GatewayIntents, Guild, GuildId, UserId};
use serenity::{Client, async_trait};
use songbird::driver::opus::{Channels as OpusChannels, Decoder};
use songbird::driver::{Channels, DecodeConfig, DecodeMode, SampleRate};
use songbird::events::{CoreEvent, Event, EventContext, EventHandler as VoiceHandler};
use songbird::input::{Input, RawAdapter};
use songbird::packet::Packet;
use songbird::{Call, SerenityInit};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SILENCE: Duration = Duration::from_millis(600);
/// Transport suffix of both Discord rtpsize AEAD modes (16-byte tag + 4-byte nonce).
/// A larger `payload_end_pad` on an RtpPacket means songbird stripped a DAVE trailer.
const TRANSPORT_SUFFIX: usize = 20;

struct Sentence {
    user: u64,
    started: Instant,
    last_pkt: Instant,
    last_seq: u16,
    own: Vec<i16>,
    songbird: Vec<i16>,
    packets: u32,
    dave: u32,
    seq_gaps: u32,
    own_errors: u32,
    songbird_lost: u32,
}

#[derive(Default)]
struct Totals {
    sentences: u32,
    packets: u64,
    dave: u64,
    seq_gaps: u64,
    own_errors: u64,
    songbird_lost: u64,
    unmapped: u64,
    speaking_events: u64,
    rtp_events: u64,
    ticks_with_voice: u64,
}

#[derive(Default)]
struct State {
    ssrc_user: HashMap<u32, u64>,
    decoders: HashMap<u32, Decoder>,
    open: HashMap<u32, Sentence>,
    totals: Totals,
}

type Shared = Arc<Mutex<State>>;

#[derive(Clone)]
struct Receiver(Shared);

#[async_trait]
impl VoiceHandler for Receiver {
    async fn act(&self, ctx: &EventContext<'_>) -> Option<Event> {
        let mut st = self.0.lock().unwrap();
        match ctx {
            EventContext::SpeakingStateUpdate(s) => {
                st.totals.speaking_events += 1;
                println!("[voice] speaking update: ssrc {} user {:?} {:?}", s.ssrc, s.user_id, s.speaking);
                if let Some(u) = s.user_id {
                    st.ssrc_user.insert(s.ssrc, u.0);
                }
            },
            EventContext::RtpPacket(p) => {
                st.totals.rtp_events += 1;
                let rtp = p.rtp();
                let ssrc = rtp.get_ssrc();
                let seq: u16 = rtp.get_sequence().into();
                let payload = rtp.payload();
                let opus = &payload[p.payload_offset..payload.len() - p.payload_end_pad];
                let dave = p.payload_end_pad > TRANSPORT_SUFFIX;
                let Some(&user) = st.ssrc_user.get(&ssrc) else {
                    st.totals.unmapped += 1;
                    return None;
                };
                let mut pcm = vec![0i16; 5760 * 2]; // 120 ms stereo, Opus' largest frame
                let decoded = st
                    .decoders
                    .entry(ssrc)
                    .or_insert_with(|| Decoder::new(48000, OpusChannels::Stereo).unwrap())
                    .decode(opus, &mut pcm, false);
                let now = Instant::now();
                let s = st.open.entry(ssrc).or_insert_with(|| Sentence {
                    user,
                    started: now,
                    last_pkt: now,
                    last_seq: seq.wrapping_sub(1),
                    own: Vec::new(),
                    songbird: Vec::new(),
                    packets: 0,
                    dave: 0,
                    seq_gaps: 0,
                    own_errors: 0,
                    songbird_lost: 0,
                });
                // missed sequence numbers: network loss, or packets songbird dropped silently (DAVE not ready / failed)
                s.seq_gaps += u32::from(seq.wrapping_sub(s.last_seq).wrapping_sub(1).min(1000));
                s.last_seq = seq;
                s.last_pkt = now;
                s.packets += 1;
                s.dave += u32::from(dave);
                match decoded {
                    Ok(n) => s.own.extend_from_slice(&pcm[..n * 2]),
                    Err(_) => s.own_errors += 1,
                }
            },
            EventContext::VoiceTick(t) => {
                st.totals.ticks_with_voice += u64::from(!t.speaking.is_empty());
                for (ssrc, data) in &t.speaking {
                    if let Some(s) = st.open.get_mut(ssrc) {
                        if data.packet.is_none() {
                            s.songbird_lost += 1;
                        }
                        if let Some(v) = &data.decoded_voice {
                            s.songbird.extend_from_slice(v);
                        }
                    }
                }
            },
            EventContext::DriverConnect(c) => println!("[voice] driver connected: ssrc {}, server {}", c.ssrc, c.server),
            EventContext::DriverReconnect(_) => println!("[voice] driver reconnected"),
            EventContext::DriverDisconnect(d) => println!("[voice] driver disconnected: {:?}", d.reason),
            _ => {},
        }
        None
    }
}

fn write_wav(path: &PathBuf, pcm: &[i16]) {
    let spec = hound::WavSpec { channels: 2, sample_rate: 48000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).expect("create wav");
    for &x in pcm {
        w.write_sample(x).unwrap();
    }
    w.finalize().unwrap();
}

/// 120 ms, 880 Hz, f32 stereo: the same beep as bot.js.
fn beep() -> Input {
    let mut bytes = Vec::new();
    for i in 0..5760 {
        let v = (2.0 * std::f32::consts::PI * 880.0 * i as f32 / 48000.0).sin() * 0.18 * ((5760 - i) as f32 / 1500.0).min(1.0);
        bytes.extend_from_slice(&v.to_le_bytes());
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    RawAdapter::new(std::io::Cursor::new(bytes), 48000, 2).into()
}

fn print_totals(t: &Totals) {
    let pct = |x: u64| 100.0 * x as f64 / t.packets.max(1) as f64;
    println!(
        "[totals] sentences={} packets={} dave_decrypted={} ({:.1}%) seq_gaps={} ({:.2}%) own_decode_errors={} ({:.2}%) songbird_lost_ticks={} unmapped_packets={}",
        t.sentences, t.packets, t.dave, pct(t.dave), t.seq_gaps, pct(t.seq_gaps), t.own_errors, pct(t.own_errors), t.songbird_lost, t.unmapped
    );
}

/// Closes sentences after 600 ms without packets; writes both WAVs and beeps.
async fn cutter(st: Shared, call: Arc<tokio::sync::Mutex<Call>>, out: PathBuf) {
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        let done: Vec<Sentence> = {
            let mut g = st.lock().unwrap();
            let ended: Vec<u32> = g.open.iter().filter(|(_, s)| s.last_pkt.elapsed() >= SILENCE).map(|(k, _)| *k).collect();
            ended.into_iter().filter_map(|k| g.open.remove(&k)).collect()
        };
        for s in done {
            let n = {
                let t = &mut st.lock().unwrap().totals;
                t.sentences += 1;
                t.packets += u64::from(s.packets);
                t.dave += u64::from(s.dave);
                t.seq_gaps += u64::from(s.seq_gaps);
                t.own_errors += u64::from(s.own_errors);
                t.songbird_lost += u64::from(s.songbird_lost);
                t.sentences
            };
            write_wav(&out.join(format!("{n:03}-own.wav")), &s.own);
            write_wav(&out.join(format!("{n:03}-songbird.wav")), &s.songbird);
            println!(
                "[sentence {n:03}] user {} {:.1}s packets={} dave={} seq_gaps={} own_decode_errors={} songbird_lost_ticks={} own={:.2}s songbird={:.2}s",
                s.user,
                (s.last_pkt - s.started).as_secs_f32(),
                s.packets,
                s.dave,
                s.seq_gaps,
                s.own_errors,
                s.songbird_lost,
                s.own.len() as f32 / 96000.0,
                s.songbird.len() as f32 / 96000.0
            );
            print_totals(&st.lock().unwrap().totals); // also here: a killed process never reaches Ctrl+C
            call.lock().await.play_input(beep());
        }
    }
}

struct Bot {
    guild: GuildId,
    target: UserId,
    joined: AtomicBool,
    state: Shared,
    out: PathBuf,
}

#[async_trait]
impl EventHandler for Bot {
    async fn guild_create(&self, ctx: Context, guild: Guild, _: Option<bool>) {
        if guild.id != self.guild || self.joined.swap(true, Ordering::SeqCst) {
            return;
        }
        let Some(channel): Option<ChannelId> = guild.voice_states.get(&self.target).and_then(|v| v.channel_id) else {
            println!("[voice] user {} is not in a voice channel of this server: join one and restart", self.target);
            std::process::exit(2);
        };
        let manager = songbird::get(&ctx).await.unwrap();
        // Handlers go on the Call BEFORE joining: Discord sends each present user's Speaking (SSRC -> user)
        // once, right at connect; registering after join() returns misses it and no packet can be attributed.
        let call = manager.get_or_insert(self.guild);
        {
            let mut c = call.lock().await;
            let r = Receiver(self.state.clone());
            for ev in [
                CoreEvent::SpeakingStateUpdate,
                CoreEvent::RtpPacket,
                CoreEvent::VoiceTick,
                CoreEvent::DriverConnect,
                CoreEvent::DriverReconnect,
                CoreEvent::DriverDisconnect,
            ] {
                c.add_global_event(ev.into(), r.clone());
            }
        }
        let t0 = Instant::now();
        if let Err(e) = manager.join(self.guild, channel).await {
            println!("[voice] join FAILED: {e:?}");
            std::process::exit(3);
        }
        println!("[voice] joined channel {channel} in {} ms", t0.elapsed().as_millis());
        call.lock().await.play_input(beep());
        let st = self.state.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(10)).await;
                let g = st.lock().unwrap();
                let t = &g.totals;
                println!("[raw] speaking_events={} rtp_events={} ticks_with_voice={} unmapped={} known_ssrcs={}", t.speaking_events, t.rtp_events, t.ticks_with_voice, t.unmapped, g.ssrc_user.len());
            }
        });
        tokio::spawn(cutter(self.state.clone(), call, self.out.clone()));
        println!("[voice] listening; Ctrl+C to stop and print totals");
    }
}

/// Reads KEY=value lines from the main checkout's .env (never printed).
fn env_file(path: &str) -> HashMap<String, String> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().trim_matches('"').to_string()))
        .collect()
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().with_env_filter(
        tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn,songbird=info".into()),
    ).init();
    let env_path = std::env::var("BOBBY_ENV")
        .unwrap_or_else(|_| r"C:\Users\alexa\Desktop\Personal Projects\discord-bot\.env".into());
    let env = env_file(&env_path);
    let token = &env["DISCORD_TOKEN"];
    let guild = GuildId::new(env["GUILD_ID"].parse().expect("GUILD_ID"));
    // target: first CLI argument, else the first super admin
    let target = std::env::args()
        .nth(1)
        .or_else(|| env.get("SUPER_ADMINS").and_then(|s| s.split(',').next()).map(|s| s.trim().to_string()))
        .expect("pass a user ID or set SUPER_ADMINS");
    let target = UserId::new(target.parse().expect("user ID"));
    let out = std::env::var("R0_OUT").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("../../../_logs/r0"));
    std::fs::create_dir_all(&out).unwrap();

    let config = songbird::Config::default()
        .decode_mode(DecodeMode::Decode(DecodeConfig::new(Channels::Stereo, SampleRate::Hz48000)));
    let state: Shared = Arc::default();
    let bot = Bot { guild, target, joined: AtomicBool::new(false), state: state.clone(), out };
    let mut client = Client::builder(token, GatewayIntents::GUILDS | GatewayIntents::GUILD_VOICE_STATES)
        .event_handler(bot)
        .register_songbird_from_config(config)
        .await
        .expect("client");
    tokio::select! {
        r = client.start() => println!("[gateway] stopped: {r:?}"),
        _ = tokio::signal::ctrl_c() => {},
    }
    print_totals(&state.lock().unwrap().totals);
}
