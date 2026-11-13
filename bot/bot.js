const path = require('path');
require('dotenv').config({ path: path.join(__dirname, '..', '.env'), quiet: true });
const fs = require('fs');
const { Readable, PassThrough } = require('stream');

// Mirror the console to _logs/bot.log, so errors can be reviewed later.
fs.mkdirSync(path.join(__dirname, '..', '_logs'), { recursive: true });
const LOG = fs.createWriteStream(path.join(__dirname, '..', '_logs', 'bot.log'), { flags: 'a' });
for (const level of ['log', 'error']) {
  const original = console[level];
  console[level] = (...args) => {
    original(...args);
    LOG.write(`${new Date().toISOString()} ${args.map((a) => (a instanceof Error ? a.stack : typeof a === 'string' ? a : JSON.stringify(a))).join(' ')}
`);
  };
}
const { Client, GatewayIntentBits, Events } = require('discord.js');
const {
  joinVoiceChannel, getVoiceConnection, entersState, EndBehaviorType, VoiceConnectionStatus,
  createAudioPlayer, createAudioResource, StreamType,
} = require('@discordjs/voice');
const { OpusEncoder } = require('@discordjs/opus');
const brain = require('./brain');
const claude = require('./actions/ask');
const { Mixer } = require('./music/mixer');
const { Player } = require('./music/player');
const { decode } = require('./music/sources');
const aliases = require('./actions/aliases');
const roles = require('./roles');
const { whenAlone } = require('./actions/discord');
const { messages } = require('./format');

const VOICE_URL = process.env.VOICE_URL || 'http://127.0.0.1:3000';
// Silence that ends a sentence. Shorter is snappier, but cuts you off if you pause mid-sentence.
const SILENCE_MS = parseInt(process.env.RECORD_SILENCE_MS || '600');
// Text channel (name or ID) where Bobby writes what it says. Empty: the channel where /join was typed.
const TEXT_CHANNEL = process.env.TEXT_CHANNEL || '';

// allowedMentions: a YouTube title or nickname like "@everyone" must never ping the server.
const client = new Client({ intents: [GatewayIntentBits.Guilds, GatewayIntentBits.GuildVoiceStates], allowedMentions: { parse: [] } });
// Last safety net: a failed reply (voice service restarting, expired interaction) is logged, never fatal.
process.on('unhandledRejection', (e) => console.error('[unhandled]', e));
const player = createAudioPlayer(); // ponytail: one player, Bobby lives on a single server
// Everything audible goes through the mixer: music underneath, beep/TTS on top (music ducks meanwhile).
const mixer = new Mixer((out) => player.play(createAudioResource(out, { inputType: StreamType.Raw })));
const music = new Player(mixer);

// 120 ms beep at 880 Hz, 48 kHz stereo PCM, generated once.
const BEEP = (() => {
  const n = 5760, b = Buffer.alloc(n * 4);
  for (let i = 0; i < n; i++) {
    const v = Math.round(Math.sin((2 * Math.PI * 880 * i) / 48000) * 6000 * Math.min(1, (n - i) / 1500));
    b.writeInt16LE(v, i * 4); b.writeInt16LE(v, i * 4 + 2);
  }
  return b;
})();

// "Heard Bobby, did not get it": two soft descending tones (330 Hz then 220 Hz), 250 ms, raised-cosine envelope per tone (no clicks).
const NOPE = (() => {
  const seg = 6000, b = Buffer.alloc(seg * 2 * 4);
  [330, 220].forEach((hz, k) => {
    for (let i = 0; i < seg; i++) {
      const v = Math.round(Math.sin((2 * Math.PI * hz * i) / 48000) * 4500 * Math.sin((Math.PI * i) / seg) ** 2);
      b.writeInt16LE(v, (k * seg + i) * 4); b.writeInt16LE(v, (k * seg + i) * 4 + 2);
    }
  });
  return b;
})();

// Spoken acks, synthesized once by the voice service (retried while it was not up yet).
const ACKS = ["D'accord.", 'Ça marche.', "Je m'en occupe.", "C'est parti.", 'Entendu.'];
const ackPcm = new Map();
let lastAck = null, ackLoading = false;
async function loadAcks() {
  if (ackLoading || ackPcm.size === ACKS.length) return;
  ackLoading = true;
  for (const t of ACKS) {
    if (ackPcm.has(t)) continue;
    try {
      const res = await fetch(`${VOICE_URL}/tts?lang=fr&voice=bobby&text=${encodeURIComponent(t)}`);
      if (!res.ok) throw new Error(`tts ${res.status}`);
      const chunks = [];
      for await (const c of decode(Readable.fromWeb(res.body))) chunks.push(c);
      ackPcm.set(t, Buffer.concat(chunks));
    } catch { break; }
  }
  ackLoading = false;
  if (ackPcm.size < ACKS.length) setTimeout(loadAcks, 10_000);
}

// 0.4 s of silence after a spoken reply: Discord cuts the end of a stream, this keeps it off the last syllable.
const TAIL = Buffer.alloc(48000 * 4 * 0.4);

// Moves the live connection to another channel of the same server: joinVoiceChannel on an existing connection
// only sends a new voice state, so the receiver, the speaking listener, the player subscription (music) and
// DAVE carry over. Returns the channel Bobby left (for undo), or null when it did not move.
function follow(channel) {
  const conn = channel && getVoiceConnection(channel.guild.id);
  if (!conn || conn.joinConfig.channelId === channel.id) return null;
  const prev = channel.guild.channels.cache.get(conn.joinConfig.channelId) || null;
  joinVoiceChannel({ channelId: channel.id, guildId: channel.guild.id, adapterCreator: channel.guild.voiceAdapterCreator, selfDeaf: false });
  conn.joinConfig.channelId = channel.id; // a live connection only sends the payload: keep joinConfig (rejoins, checkAlone) in step
  console.log(`[voice] Bobby moves to ${channel.name}`);
  return prev;
}

// Never alone: when its channel empties, Bobby joins the person who typed /join; if they are not in voice,
// the music pauses and Bobby leaves after 2 minutes unless someone comes back.
const ALONE_LEAVE_MS = 120_000;
let session = null; // { guild, joinerId } since the last /join
let settle = null, leaveTimer = null, autoPaused = null; // autoPaused: the track paused because Bobby was alone
client.on(Events.VoiceStateUpdate, (o, n) => {
  if (!session || n.guild.id !== session.guild.id) return;
  clearTimeout(settle);
  settle = setTimeout(checkAlone, 1500); // a group move fires several updates: decide once they settle
});
function checkAlone() {
  const { guild, joinerId } = session || {};
  const conn = guild && getVoiceConnection(guild.id);
  if (!conn) { clearTimeout(leaveTimer); leaveTimer = null; return; }
  const voice = [...guild.voiceStates.cache.values()].filter((v) => v.channelId && v.member && !v.member.user.bot).map((v) => ({ id: v.id, channel: v.channelId }));
  const next = whenAlone({ botChannel: conn.joinConfig.channelId, joinerId, voice });
  if (next === 'wait') {
    if (leaveTimer) return;
    if (music.current && !music.paused) { music.pause(); autoPaused = music.current; }
    console.log('[voice] alone, nobody to follow: leaving in 2 minutes unless someone comes back');
    leaveTimer = setTimeout(() => { leaveTimer = null; disconnect(guild.id); console.log('[voice] left: alone for 2 minutes'); }, ALONE_LEAVE_MS);
    return;
  }
  clearTimeout(leaveTimer); leaveTimer = null;
  if (next.follow) follow(guild.channels.cache.get(next.follow));
  // only the very track Bobby paused, still paused: a pause, stop or new song by someone meanwhile wins
  if (autoPaused && music.current === autoPaused && music.paused) music.resume();
  autoPaused = null;
}

// Leaves voice now: music, Bobby's voice and the alone timers stop with it.
function disconnect(guildId) {
  music.stop();
  speech = null; ring?.stream.destroy(); ring = null; mixer.stopOverlay();
  clearTimeout(leaveTimer); leaveTimer = null; autoPaused = null;
  getVoiceConnection(guildId)?.destroy();
}

const memberName = (guild) => async (id) => (guild.members.cache.get(id) ?? (await guild.members.fetch(id).catch(() => null)))?.displayName ?? id;

let speech = null, speechGen = 0; // Bobby's voice playing now: { id (whom it answers), gen }
let ring = null; // { stream, owner }: near-silent overlay that lights Bobby's speaking ring while someone is being heard
let clip = Promise.resolve(); // the short clip (ack, beep, low sound) playing now: a reply waits for its end

function makeIo(guild, textChannel) {
  const io = {
    music,
    memberName: memberName(guild),
    textChannel,
    follow,
    // content: text, { embeds } or a list of those, split to Discord's limits. ch: another text channel ("écris-le dans général").
    post: async (content, ch = textChannel) => { for (const m of messages(content)) await ch.send(m); return ch.name; },
    // Short clips (beep, nope, ack): a reply that follows waits for the clip's end instead of cutting it.
    beep: () => io.clip(BEEP),
    nope: () => io.clip(NOPE),
    clip(pcm) { io.ringStop(); clip = mixer.overlay(new PassThrough().end(pcm)); },
    // Spoken "got it" once the transcript is an order. False while the clips are not synthesized yet (caller falls back to the beep).
    ack() {
      const pool = ACKS.filter((t) => t !== lastAck && ackPcm.has(t));
      if (!pool.length) return false;
      lastAck = pool[Math.floor(Math.random() * pool.length)];
      io.clip(ackPcm.get(lastAck));
      return true;
    },
    // Speaking ring: Discord only shows it while audio flows, so send +-2 noise (not zeros) until ringStop.
    // ponytail: one ring for everyone (one overlay channel); a second speaker heard meanwhile shares the first one's.
    ring(on, owner) {
      if (!on) return io.ringStop(owner);
      if (ring || speech || mixer.over) return;
      const me = new Readable({ read() { const b = Buffer.alloc(3840); for (let i = 0; i < 3840; i += 2) b.writeInt16LE(Math.floor(Math.random() * 5) - 2, i); this.push(b); } });
      ring = { stream: me, owner };
      mixer.overlay(me).then(() => { if (ring?.stream === me) ring = null; });
    },
    // owner given: only that speaker's ring stops (another speaker's end of sentence leaves it on)
    ringStop(owner) {
      const r = ring;
      if (!r || (owner && r.owner !== owner)) return;
      ring = null;
      if (mixer.over) mixer.stopOverlay();
      r.stream.destroy();
    },
    // voice 'claude': Claude's answers, spoken with the second pair of Piper voices.
    // forId: the person this answers, who may interrupt it with a plain "stop" (see brain.js).
    async say(text, lang = 'fr', voice = 'bobby', forId = null) {
      // Voice only: replies are spoken, never echoed in a text channel (lists still go to TEXT_CHANNEL via post).
      console.log(`[${voice}] ${text}`);
      const me = (speech = { id: forId, gen: ++speechGen });
      try {
        const res = await fetch(`${VOICE_URL}/tts?lang=${lang}&voice=${voice}&text=${encodeURIComponent(text)}`);
        if (speech !== me) return res.body?.cancel(); // interrupted, or another reply started, while it was synthesized
        await clip; // the ack or the low sound just played ends first
        if (speech !== me) return res.body?.cancel();
        const pcm = new PassThrough(), tts = decode(Readable.fromWeb(res.body));
        tts.pipe(pcm, { end: false });
        // the silence tail also ends a broken stream, so the overlay always resolves and `speech` is released
        let tail = false;
        const end = () => { if (!tail) { tail = true; pcm.end(TAIL); } };
        tts.once('end', end).once('close', end).on('error', end);
        pcm.once('close', () => tts.destroy()); // interrupted: stop decoding too
        io.ringStop();
        await mixer.overlay(pcm);
      } catch (e) { console.error(`[tts] ${e.cause?.code || e.message}`); }
      if (speech === me) speech = null;
    },
    speaking: () => speech, // { id } while Bobby's voice plays, else null
    hush: () => { speech = null; io.ringStop(); mixer.stopOverlay(); },
    // Music stops now; the connection goes after the farewell has been spoken.
    // ponytail: fixed 3 s delay, enough for "À plus."; wait for the overlay's end if longer goodbyes appear.
    leave: () => { music.stop(); setTimeout(() => disconnect(guild.id), 3000); },
  };
  return io;
}

function listen(connection, guild, io) {
  const busy = new Set();
  connection.receiver.speaking.on('start', async (userId) => {
    if (busy.has(userId)) return;
    busy.add(userId);
    const opus = connection.receiver.subscribe(userId, { end: { behavior: EndBehaviorType.AfterSilence, duration: SILENCE_MS } });
    // Decode packet by packet: a corrupt packet is skipped instead of dropping the whole sentence.
    const decoder = new OpusEncoder(48000, 2);
    const pcm = new PassThrough();
    let bad = 0;
    opus.on('data', (packet) => {
      try { pcm.write(decoder.decode(packet)); } catch { bad++; }
    });
    opus.on('end', () => { if (bad) console.log(`[audio] skipped ${bad} corrupt packet(s)`); pcm.end(); });
    opus.on('error', (e) => { console.error(`[audio] ${e.message}`); pcm.end(); });
    let tEnd = performance.now();
    opus.once('end', () => { tEnd = performance.now() - SILENCE_MS; busy.delete(userId); });
    let ws = null;
    try {
      const member = guild.members.cache.get(userId) ?? (await guild.members.fetch(userId));
      if (member.user.bot) return opus.destroy();
      // waiting for a "yes" or a command, or Bobby is answering this person (who may say "stop"): skip the wake word gate
      const expected = brain.expecting(userId); // judged when the sentence starts: a long order must not outlive the window
      const force = expected || io.speaking()?.id === userId ? '1' : '0';
      const hotwords = encodeURIComponent(brain.hotwords(brain.context(member, guild)));
      // The sentence streams to the voice service while it is spoken: the wake word is heard mid-sentence.
      ws = new WebSocket(`${VOICE_URL.replace(/^http/, 'ws')}/listen?force=${force}&hotwords=${hotwords}`);
      ws.binaryType = 'arraybuffer';
      const queue = []; // PCM decoded before the socket is open (and the final 'end')
      const flush = () => { while (queue.length && ws.readyState === 1) ws.send(queue.shift()); };
      ws.addEventListener('open', flush);
      pcm.on('data', (b) => { queue.push(b); flush(); });
      pcm.on('end', () => { queue.push('end'); flush(); });
      let gate = null;
      const result = await new Promise((resolve, reject) => {
        ws.addEventListener('message', (ev) => {
          const m = JSON.parse(ev.data);
          if (m.type === 'wake') { io.ring(true, userId); return; }
          if (m.type === 'gate') {
            gate = m;
            io.ringStop(userId);
            // No ack here: before Whisper, Bobby cannot tell "Bobby ?" (-> "Oui ?") from an order (-> "Ça marche"),
            // and Whisper answers ~0.1 s after this point anyway (2026-11-13). The brain acks once it has the text.
            return m.wake === false ? resolve(null) : undefined;
          }
          if (m.type === 'text') resolve(m);
        });
        ws.addEventListener('error', () => reject(Object.assign(new Error('voice service not reachable'), { cause: { code: 'ECONNREFUSED' } })));
        ws.addEventListener('close', () => (gate ? resolve(null) : reject(Object.assign(new Error('voice service closed the stream'), { cause: { code: 'ECONNREFUSED' } }))));
      });
      if (gate?.wake === false) {
        console.log(`[skipped] ${member.displayName}: no "Bobby" in the audio (score ${gate.score})`);
        // metadata only: this speech was not addressed to Bobby and was never transcribed
        brain.journal({ at: new Date().toISOString(), who: member.displayName, id: member.id, outcome: 'skipped', wakeScore: gate.score, seconds: gate.seconds });
      } else if (result?.text) {
        const { text, lang, ms } = result;
        console.log(`
[heard] ${member.displayName}: "${text}" (${lang}, whisper ${ms} ms${gate?.wake ? ', wake word heard' : ''})`);
        await brain.hear({ text, lang, score: gate?.score ?? null, member, guild, tEnd, sttMs: ms, io, expected });
      }
    } catch (e) {
      console.error(`[voice] ${e.cause?.code === 'ECONNREFUSED' ? `voice service not reachable at ${VOICE_URL} (is it running?)` : e.message}`);
    } finally {
      io.ringStop();
      try { ws?.close(); } catch { /* already closed */ }
      busy.delete(userId);
    }
  });
}

client.once(Events.ClientReady, async () => {
  console.log(`Bobby online: ${client.user.tag}`);
  console.log(`[rights] role needed per action: ${Object.entries(brain.roleTable()).map(([a, r]) => `${a}=${r}`).join(' ')}`);
  claude.check();
  loadAcks();
  require('./actions/pc').warm();
  await client.application.commands.set([]); // drop global commands left by older versions: only the server ones below remain
  await client.guilds.cache.get(process.env.GUILD_ID)?.commands.set([
    { name: 'join', description: 'Bobby rejoint ton salon vocal' },
    { name: 'leave', description: 'Bobby quitte le vocal' },
    { name: 'surnoms', description: 'Qui a quel surnom (super admin)' },
  ]);
});

client.on(Events.InteractionCreate, (it) => onCommand(it).catch((e) => console.error('[command]', e)));

async function onCommand(it) {
  if (!it.isChatInputCommand()) return;
  const need = { join: 'prime', leave: 'admin' }[it.commandName] || 'super';
  if (!roles.allowed(roles.roleOf(it.member), need)) {
    return it.reply({ content: "Tu n'as pas le droit d'utiliser cette commande.", ephemeral: true });
  }
  if (it.commandName === 'leave') {
    disconnect(it.guildId);
    return it.reply('À plus.');
  }
  if (it.commandName === 'surnoms') {
    const card = await aliases.listCard({ memberName: memberName(it.guild) });
    return it.reply(card ? messages(card)[0] : "Personne n'a de surnom.");
  }
  const channel = it.member.voice.channel;
  if (!channel) return it.reply({ content: 'Rejoins un salon vocal d\'abord.', ephemeral: true });
  getVoiceConnection(it.guildId)?.destroy(); // a second /join must not attach a second listener (double actions)
  session = { guild: it.guild, joinerId: it.user.id };
  clearTimeout(leaveTimer); leaveTimer = null; autoPaused = null;
  const connection = joinVoiceChannel({
    channelId: channel.id, guildId: channel.guild.id, adapterCreator: channel.guild.voiceAdapterCreator, selfDeaf: false,
  });
  connection.subscribe(player);
  await it.reply(`Bobby écoute dans ${channel.name}. Dis « Bobby, … ».`);
  try {
    await entersState(connection, VoiceConnectionStatus.Ready, 20_000);
  } catch {
    connection.destroy();
    return it.followUp('Connexion vocale impossible.');
  }
  const text = channel.guild.channels.cache.find((c) => c.isTextBased() && (c.id === TEXT_CHANNEL || c.name === TEXT_CHANNEL)) || it.channel;
  loadAcks();
  listen(connection, channel.guild, makeIo(channel.guild, text));
}

client.login(process.env.DISCORD_TOKEN);
