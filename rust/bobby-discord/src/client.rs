//! The Discord connection: gateway, slash commands, voice join/leave, and the loop that serves the brain.
//! Port of the connection part of `bot/bot.js`.

use crate::{caps, config::Config, context, receive};
use bobby_proto::msg::{BrainMsg, CapKind, ClientKind, ClientMsg, Context as BrainContext, PROTO_VERSION};
use serenity::all::{
    ChannelId, ChannelType, Command, CommandInteraction, CreateCommand, CreateEmbed, CreateInteractionResponse,
    CreateInteractionResponseMessage, EventHandler, GatewayIntents, GuildId, Interaction, Ready, UserId,
};
use serenity::cache::Cache;
use serenity::http::Http;
use serenity::{async_trait, Client};
use songbird::driver::{Channels, DecodeConfig, DecodeMode, SampleRate};
use songbird::{SerenityInit, Songbird};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

/// How long a slash command waits for the brain before Discord's 3 s deadline gets close.
const REPLY_WAIT: Duration = Duration::from_millis(2500);

#[derive(Default)]
struct State {
    /// Where Bobby posts, remembered from the last `/join` (or `TEXT_CHANNEL`, see `default_text`).
    text_channel: Option<ChannelId>,
    joiner: Option<UserId>,
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
    next_id: AtomicU64,
}

impl Bot {
    pub fn joiner(&self) -> Option<UserId> {
        self.state.lock().unwrap().joiner
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
        }
        self.state.lock().unwrap().joiner = Some(joiner);
        let call = self.songbird.get_or_insert(self.guild);
        receive::attach(&mut *call.lock().await, self.clone());
        match tokio::time::timeout(Duration::from_secs(20), self.songbird.join(self.guild, channel)).await {
            Ok(Ok(_)) => Ok(()),
            _ => {
                let _ = self.songbird.remove(self.guild).await;
                Err("Connexion vocale impossible.".into())
            }
        }
    }

    pub async fn leave(&self) {
        let _ = self.songbird.remove(self.guild).await;
    }

    /// `TEXT_CHANNEL` if it exists, else the channel where the command was typed (`textFor` of bot.js).
    fn text_for(&self, typed_in: ChannelId) -> ChannelId {
        self.default_text().unwrap_or(typed_in)
    }

    async fn on_command(self: &Arc<Self>, cmd: CommandInteraction) {
        let user = cmd.user.id;
        let text = self.text_for(cmd.channel_id);
        if cmd.data.name == "join" {
            self.state.lock().unwrap().text_channel = Some(text);
        }
        self.send_context(user);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.state.lock().unwrap().pending.insert(id, tx);
        self.send(ClientMsg::Command { id, name: cmd.data.name.clone(), user: user.to_string(), text_channel: text.to_string() });
        let reply = tokio::time::timeout(REPLY_WAIT, rx).await;
        let mut msg = CreateInteractionResponseMessage::new();
        match reply {
            Ok(Ok(BrainMsg::Reply { text, embeds, ephemeral, .. })) => {
                msg = msg.ephemeral(ephemeral).content(text.unwrap_or_default());
                for e in embeds {
                    let mut em = CreateEmbed::new().title(e.title).description(e.description);
                    if let Some(c) = e.color {
                        em = em.color(c);
                    }
                    msg = msg.add_embed(em);
                }
            }
            _ => {
                self.state.lock().unwrap().pending.remove(&id);
                msg = msg.ephemeral(true).content("Bobby ne répond pas.");
            }
        }
        if let Err(e) = cmd.create_response(&self.http, CreateInteractionResponse::Message(msg)).await {
            eprintln!("[command] reply failed: {e}");
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
        bot.send(ClientMsg::Hello {
            v: PROTO_VERSION,
            device: "discord".into(),
            kind: ClientKind::Discord,
            caps: vec![CapKind::DiscordVoice, CapKind::DiscordText, CapKind::Music],
        });
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
    let bot = Arc::new(Bot {
        guild: GuildId::new(cfg.guild_id),
        cfg,
        http: client.http.clone(),
        cache: client.cache.clone(),
        songbird,
        to_brain,
        state: Mutex::new(State::default()),
        next_id: AtomicU64::new(1),
    });
    let _ = slot.set(bot.clone());
    tokio::spawn(bot.brain_loop(from_brain));
    client.start().await.map_err(|e| format!("gateway: {e}"))
}
