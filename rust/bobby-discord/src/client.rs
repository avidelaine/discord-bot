//! The Discord connection: gateway, slash commands, voice join/leave, and the loop that serves the brain.
//! Port of the connection part of `bot/bot.js`.

use crate::mixer::{self, MixEvent, Mixer};
use crate::player::Player;
use crate::{caps, config::Config, context, format, receive, speech};
use bobby_proto::msg::{BrainMsg, CapKind, ClientKind, ClientMsg, Context as BrainContext, PROTO_VERSION};
use serenity::all::{
    ChannelId, ChannelType, Command, CommandInteraction, CreateAllowedMentions, CreateCommand, CreateInteractionResponse,
    CreateInteractionResponseFollowup, CreateInteractionResponseMessage, EventHandler, GatewayIntents, GuildId, Interaction, Ready, UserId,
};
use serenity::cache::Cache;
use serenity::http::Http;
use serenity::{async_trait, Client};
use songbird::driver::{Channels, DecodeConfig, DecodeMode, SampleRate};
use songbird::{SerenityInit, Songbird};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

/// How long a slash command waits for the brain before Discord's 3 s deadline gets close.
const REPLY_WAIT: Duration = Duration::from_millis(2500);

#[derive(Default)]
struct State {
    /// Where Bobby posts, remembered from the last `/join` (or `TEXT_CHANNEL`, see `default_text`).
    text_channel: Option<ChannelId>,
    joiner: Option<UserId>,
    /// Who typed the last `/join`, and when (see `Cap::Join`).
    last_join_cmd: Option<(UserId, Instant)>,
    pending: HashMap<u64, oneshot::Sender<BrainMsg>>,
    /// Last context sent, so an unchanged one is not resent before every utterance.
    last_ctx: Option<BrainContext>,
}

pub struct Bot {
    pub cfg: Config,
    pub guild: GuildId,
    pub http: Arc<Http>,
    pub cache: Arc<Cache>,
    pub songbird: Arc<Songbird>,
    pub to_brain: UnboundedSender<ClientMsg>,
    state: Mutex<State>,
    /// Who is heard without a wake word, and who Bobby is answering (see `receive`).
    pub listen: std::sync::Mutex<receive::Listen>,
    /// Everything audible: music underneath, beeps and Bobby's voice on top (see `mixer`).
    pub mixer: Arc<Mutex<Mixer>>,
    pub player: Player,
    /// Beep/ack/ring/TTS state (see `speech`).
    pub speech: std::sync::Mutex<crate::speech::Speech>,
    /// Debounce and 2 minute leave timers of the never-alone behaviour (see `alone`).
    pub alone: std::sync::Mutex<crate::alone::Alone>,
    next_id: AtomicU64,
    /// The mixer's songbird track: a duplicate `Start` replaces it instead of adding a second puller.
    mix_track: Mutex<Option<songbird::tracks::TrackHandle>>,
    /// `ready` fires again on a gateway re-identify: Hello and acks go out once per process.
    hello_sent: AtomicBool,
}

impl Bot {
    pub fn joiner(&self) -> Option<UserId> {
        self.state.lock().unwrap().joiner
    }

    pub fn last_join_cmd(&self) -> Option<(UserId, Instant)> {
        self.state.lock().unwrap().last_join_cmd
    }

    /// Who typed /join, for the never-alone behaviour (the brain sets it when it joins on someone's behalf).
    pub fn set_joiner(&self, u: UserId) {
        self.state.lock().unwrap().joiner = Some(u);
    }

    pub fn send(&self, msg: ClientMsg) {
        let _ = self.to_brain.send(msg);
    }

    /// Text channel Bobby posts in: `TEXT_CHANNEL` (name or id) if it exists in the guild, else the remembered one.
    pub fn default_text(&self) -> Option<ChannelId> {
        let configured = self.cache.guild(self.guild).and_then(|g| {
            g.channels
                .values()
                .find(|c| c.kind == ChannelType::Text && (c.id.to_string() == self.cfg.text_channel || c.name == self.cfg.text_channel))
                .map(|c| c.id)
        });
        configured.or(self.state.lock().unwrap().text_channel)
    }

    /// Builds the context for `speaker` from the cache and sends it if it differs from the last one sent.
    /// Call before each `Heard` so the brain never reasons on a stale server.
    pub fn send_context(&self, speaker: UserId) {
        let bobby = self.default_text().map(|c| c.to_string());
        let Some(ctx) = self.cache.guild(self.guild).map(|g| context::from_guild(&g, speaker.get(), bobby)) else { return };
        let mut st = self.state.lock().unwrap();
        if st.last_ctx.as_ref() != Some(&ctx) {
            st.last_ctx = Some(ctx.clone());
            drop(st);
            self.send(ClientMsg::Context(ctx));
        }
    }

    /// Joins `channel` and listens there. Handlers go on the call BEFORE the join (see `receive::attach`).
    pub async fn join(self: &Arc<Self>, channel: ChannelId, joiner: UserId) -> Result<(), String> {
        // A second join must not stack a second listener (double actions): start from a fresh call.
        if self.songbird.get(self.guild).is_some() {
            let _ = self.songbird.remove(self.guild).await;
            self.mixer.lock().unwrap().detach();
        }
        self.state.lock().unwrap().joiner = Some(joiner);
        crate::alone::reset(self);
        let call = self.songbird.get_or_insert(self.guild);
        receive::attach(&mut *call.lock().await, self.clone());
        match tokio::time::timeout(Duration::from_secs(20), self.songbird.join(self.guild, channel)).await {
            Ok(Ok(_)) => {
                // Whatever the mixer had to play before this call existed starts now.
                self.mixer.lock().unwrap().restart();
                Ok(())
            }
            _ => {
                let _ = self.songbird.remove(self.guild).await;
                self.mixer.lock().unwrap().detach();
                Err("Connexion vocale impossible.".into())
            }
        }
    }

    /// Moves the live call to another channel (`follow` of bot.js): songbird's join on an existing call only
    /// sends a new voice state, so the receive handlers and the mixer track carry over. Returns the channel
    /// Bobby left (for undo), or None when it was not connected or already there.
    pub async fn move_to(&self, channel: ChannelId) -> Option<ChannelId> {
        let call = self.songbird.get(self.guild)?;
        let prev = call.lock().await.current_channel().map(|c| ChannelId::new(c.0.get()))?;
        if prev == channel {
            return None;
        }
        match tokio::time::timeout(Duration::from_secs(20), self.songbird.join(self.guild, channel)).await {
            Ok(Ok(_)) => {
                eprintln!("[voice] Bobby moves to {channel}");
                Some(prev)
            }
            _ => {
                eprintln!("[voice] move to {channel} failed");
                None
            }
        }
    }

    pub async fn leave(self: &Arc<Self>) {
        crate::alone::reset(self);
        speech::stop_all(self);
        let _ = self.songbird.remove(self.guild).await;
        self.mixer.lock().unwrap().detach();
    }

    /// `TEXT_CHANNEL` if it exists, else the channel where the command was typed (`textFor` of bot.js).
    fn text_for(&self, typed_in: ChannelId) -> ChannelId {
        self.default_text().unwrap_or(typed_in)
    }

    async fn on_command(self: &Arc<Self>, cmd: CommandInteraction) {
        let user = cmd.user.id;
        let text = self.text_for(cmd.channel_id);
        if cmd.data.name == "join" {
            let mut st = self.state.lock().unwrap();
            st.text_channel = Some(text);
            st.last_join_cmd = Some((user, Instant::now()));
        }
        self.send_context(user);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.state.lock().unwrap().pending.insert(id, tx);
        self.send(ClientMsg::Command { id, name: cmd.data.name.clone(), user: user.to_string(), text_channel: text.to_string() });
        let reply = tokio::time::timeout(REPLY_WAIT, rx).await;
        let (outs, ephemeral) = match reply {
            Ok(Ok(BrainMsg::Reply { text, embeds, ephemeral, .. })) => (format::messages(text.as_deref(), &embeds), ephemeral),
            _ => {
                self.state.lock().unwrap().pending.remove(&id);
                (vec![format::Out { content: Some("Bobby ne répond pas.".into()), embeds: vec![] }], true)
            }
        };
        let mut outs = outs.into_iter();
        let first = outs.next().unwrap_or_default();
        let mut msg = CreateInteractionResponseMessage::new()
            .ephemeral(ephemeral)
            .allowed_mentions(CreateAllowedMentions::new()); // empty = parse nothing
        let embeds = format::to_embeds(&first);
        msg = match first.content {
            Some(c) => msg.content(c),
            None if embeds.is_empty() => msg.content("…"),
            None => msg,
        };
        if !embeds.is_empty() {
            msg = msg.embeds(embeds);
        }
        if let Err(e) = cmd.create_response(&self.http, CreateInteractionResponse::Message(msg)).await {
            eprintln!("[command] reply failed: {e}");
            return;
        }
        for o in outs {
            let mut f = CreateInteractionResponseFollowup::new().ephemeral(ephemeral).allowed_mentions(CreateAllowedMentions::new());
            let embeds = format::to_embeds(&o);
            if let Some(c) = o.content {
                f = f.content(c);
            }
            if !embeds.is_empty() {
                f = f.embeds(embeds);
            }
            if let Err(e) = cmd.create_followup(&self.http, f).await {
                eprintln!("[command] follow-up failed: {e}");
            }
        }
    }

    /// Mixer events: (re)start the songbird track pulling the mixer, tell the player a track ended.
    async fn mix_events(self: Arc<Self>, mut rx: UnboundedReceiver<MixEvent>) {
        while let Some(ev) = rx.recv().await {
            match ev {
                MixEvent::Start => match self.songbird.get(self.guild) {
                    Some(call) => {
                        // Stop the previous puller first: two inputs on one mixer play at double speed.
                        let mut c = call.lock().await;
                        let old = self.mix_track.lock().unwrap().take();
                        if let Some(old) = old {
                            let _ = old.stop();
                        }
                        let h = c.play_input(mixer::input(self.mixer.clone()));
                        *self.mix_track.lock().unwrap() = Some(h);
                    }
                    // Not in voice: nobody pulls frames. `restart` after the next join plays what is left.
                    None => self.mixer.lock().unwrap().detach(),
                },
                // Spawned: on_ended may wait on yt-dlp for 20 s and Starts must not queue behind it.
                MixEvent::MusicEnded { tag, read } => {
                    let bot = self.clone();
                    tokio::spawn(async move { bot.player.on_ended(tag, read).await });
                }
            }
        }
    }

    /// Serves the brain: replies to pending commands, runs caps, forwards the rest.
    async fn brain_loop(self: Arc<Self>, mut from_brain: UnboundedReceiver<BrainMsg>) {
        while let Some(msg) = from_brain.recv().await {
            match msg {
                BrainMsg::Reply { id, .. } => {
                    let tx = self.state.lock().unwrap().pending.remove(&id);
                    if let Some(tx) = tx {
                        let _ = tx.send(msg);
                    }
                }
                // Spawned: a slow cap (a join takes seconds) must not block the next message.
                BrainMsg::Run { id, cap } => {
                    let bot = self.clone();
                    tokio::spawn(async move {
                        let result = caps::run(&bot, id, cap).await;
                        bot.send(result);
                    });
                }
                other => caps::brain_msg(&self, other).await,
            }
        }
    }
}

struct Handler(Arc<OnceLock<Arc<Bot>>>);

#[async_trait]
impl EventHandler for Handler {
    async fn ready(&self, _: serenity::all::Context, ready: Ready) {
        let Some(bot) = self.0.get() else { return };
        eprintln!("[discord] online as {}", ready.user.name);
        if bot.hello_sent.swap(true, Ordering::SeqCst) {
            return;
        }
        bot.send(ClientMsg::Hello {
            v: PROTO_VERSION,
            device: "discord".into(),
            kind: ClientKind::Discord,
            caps: vec![CapKind::DiscordVoice, CapKind::DiscordText, CapKind::Music, CapKind::Voice],
        });
        speech::spawn_acks(bot.clone());
        // Drop global commands left by older versions: only the server ones below remain.
        if let Err(e) = Command::set_global_commands(&bot.http, vec![]).await {
            eprintln!("[discord] clearing global commands failed: {e}");
        }
        let cmds = vec![
            CreateCommand::new("join").description("Bobby rejoint ton salon vocal"),
            CreateCommand::new("leave").description("Bobby quitte le vocal"),
            CreateCommand::new("surnoms").description("Qui a quel surnom (super admin)"),
        ];
        if let Err(e) = bot.guild.set_commands(&bot.http, cmds).await {
            eprintln!("[discord] registering commands failed: {e}");
        }
    }

    async fn voice_state_update(&self, _: serenity::all::Context, _old: Option<serenity::all::VoiceState>, new: serenity::all::VoiceState) {
        if let Some(bot) = self.0.get() {
            if new.guild_id == Some(bot.guild) {
                crate::alone::on_voice_update(bot);
            }
        }
    }

    async fn interaction_create(&self, _: serenity::all::Context, it: Interaction) {
        if let (Interaction::Command(cmd), Some(bot)) = (it, self.0.get()) {
            bot.on_command(cmd).await;
        }
    }
}

/// Connects to Discord and serves the brain until the gateway stops.
pub async fn run(cfg: Config, from_brain: UnboundedReceiver<BrainMsg>, to_brain: UnboundedSender<ClientMsg>) -> Result<(), String> {
    // Decoded PCM per speaker is what the receive side consumes (same setup as the R0 spike).
    let sb_config = songbird::Config::default().decode_mode(DecodeMode::Decode(DecodeConfig::new(Channels::Stereo, SampleRate::Hz48000)));
    let songbird = Songbird::serenity_from_config(sb_config);
    let slot = Arc::new(OnceLock::new());
    let mut client = Client::builder(&cfg.token, GatewayIntents::GUILDS | GatewayIntents::GUILD_VOICE_STATES)
        .event_handler(Handler(slot.clone()))
        .register_songbird_with(songbird.clone())
        .await
        .map_err(|e| format!("discord client: {e}"))?;
    if bobby_infer::is_local(&cfg.voice_url) {
        // Models load in the background so the gateway starts at once; callers tolerate "not loaded yet".
        tokio::spawn(load_local_engines(cfg.models_dir.clone()));
    }
    let (mix_tx, mix_rx) = tokio::sync::mpsc::unbounded_channel();
    let mixer = Arc::new(Mutex::new(Mixer::new(mix_tx)));
    let bot = Arc::new(Bot {
        guild: GuildId::new(cfg.guild_id),
        cfg,
        http: client.http.clone(),
        cache: client.cache.clone(),
        songbird,
        to_brain,
        state: Mutex::new(State::default()),
        listen: Default::default(),
        speech: Default::default(),
        alone: Default::default(),
        player: Player::new(mixer.clone()),
        mixer,
        next_id: AtomicU64::new(1),
        mix_track: Mutex::new(None),
        hello_sent: AtomicBool::new(false),
    });
    let _ = slot.set(bot.clone());
    tokio::spawn(bot.clone().mix_events(mix_rx));
    tokio::spawn(bot.brain_loop(from_brain));
    client.start().await.map_err(|e| format!("gateway: {e}"))
}

/// Loads Whisper, the wake word and Piper in process (slow: once, off the async threads). A failure is
/// logged once and leaves the engine unset, which behaves like an unreachable voice service.
async fn load_local_engines(models: std::path::PathBuf) {
    use bobby_infer::listen::{wake_threshold, Listener};
    let loaded = tokio::task::spawn_blocking(move || {
        let stt = bobby_infer::Stt::from_addr("local", &models).map_err(|e| format!("stt: {e}"))?;
        let root = models.parent().map(|p| p.to_path_buf());
        let listener = Listener::new(Arc::new(stt), Some(&models.join("wakeword")), wake_threshold(), root);
        let tts = bobby_infer::Tts::from_addr("local", &models).map_err(|e| format!("tts: {e}"))?;
        Ok::<_, String>((listener, tts))
    })
    .await
    .map_err(|e| e.to_string())
    .and_then(|r| r);
    match loaded {
        Ok((listener, tts)) => {
            let _ = receive::LOCAL_LISTENER.set(Arc::new(listener));
            let _ = crate::speech::LOCAL_TTS.set(tts);
        }
        Err(e) => eprintln!("[voice] cannot load the local engines: {e}"),
    }
}
