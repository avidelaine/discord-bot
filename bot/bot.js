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

// Moves the live connection to another channel of the same server: joinVoiceChannel on an existing connection
// only sends a new voice state, so the receiver, the speaking listener, the player subscription (music) and
// DAVE carry over. Returns the channel Bobby left (for undo), or null when it did not move.
function follow(channel) {
  const conn = channel && getVoiceConnection(channel.guild.id);
  if (!conn || conn.joinConfig.channelId === channel.id) return null;
  const prev = channel.guild.channels.cache.get(conn.joinConfig.channelId) || null;
  joinVoiceChannel({ channelId: channel.id, guildId: channel.guild.id, adapterCreator: channel.guild.voiceAdapterCreator, selfDeaf: false });
  console.log(`[voice] Bobby moves to ${channel.name}`);
  return prev;
}

// Never alone: when its channel empties, Bobby joins the person who typed /join; if they are not in voice,
// the music pauses and Bobby leaves after 2 minutes unless someone comes back.
const ALONE_LEAVE_MS = 120_000;
let session = null; // { guild, joinerId } since the last /join
let settle = null, leaveTimer = null, autoPaused = false;
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
    if (music.current && !music.paused) { music.pause(); autoPaused = true; }
    console.log('[voice] alone, nobody to follow: leaving in 2 minutes unless someone comes back');
    leaveTimer = setTimeout(() => { leaveTimer = null; music.stop(); autoPaused = false; getVoiceConnection(guild.id)?.destroy(); console.log('[voice] left: alone for 2 minutes'); }, ALONE_LEAVE_MS);
    return;
  }
  clearTimeout(leaveTimer); leaveTimer = null;
  if (next.follow) follow(guild.channels.cache.get(next.follow));
  if (autoPaused) { autoPaused = false; music.resume(); }
}

const memberName = (guild) => async (id) => (guild.members.cache.get(id) ?? (await guild.members.fetch(id).catch(() => null)))?.displayName ?? id;

function makeIo(guild, textChannel) {
  return {
    music,
    memberName: memberName(guild),
    textChannel,
    follow,
    post: async (text) => { await textChannel.send(text); return textChannel.name; },
    beep: () => { mixer.overlay(new PassThrough().end(BEEP)); },
    // voice 'claude': Claude's answers, spoken with the second pair of Piper voices
    async say(text, lang = 'fr', voice = 'bobby') {
      // Voice only: replies are spoken, never echoed in a text channel (lists still go to TEXT_CHANNEL via post).
      console.log(`[${voice}] ${text}`);
      try {
        const res = await fetch(`${VOICE_URL}/tts?lang=${lang}&voice=${voice}&text=${encodeURIComponent(text)}`);
        mixer.overlay(decode(Readable.fromWeb(res.body)));
      } catch (e) { console.error(`[tts] ${e.cause?.code || e.message}`); }
    },
    // Music stops now; the connection goes after the farewell has been spoken.
    // ponytail: fixed 3 s delay, enough for "À plus."; wait for the overlay's end if longer goodbyes appear.
    leave: () => { music.stop(); setTimeout(() => getVoiceConnection(guild.id)?.destroy(), 3000); },
  };
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
    try {
      const member = guild.members.cache.get(userId) ?? (await guild.members.fetch(userId));
      if (member.user.bot) return opus.destroy();
      const res = await fetch(`${VOICE_URL}/transcribe`, {
        method: 'POST', body: pcm, duplex: 'half',
        headers: {
          'x-hotwords': encodeURIComponent(brain.hotwords(brain.context(member, guild))),
          'x-force': brain.expecting(userId) ? '1' : '0', // waiting for a "yes" or a command: skip the wake word gate
        },
      });
      const { text, lang, wake, score, seconds, ms } = await res.json();
      if (text) console.log(`\n[heard] ${member.displayName}: "${text}" (${lang}, whisper ${ms} ms${wake ? ', wake word heard' : ''})`);
      else if (wake === false) {
        console.log(`[skipped] ${member.displayName}: no "Bobby" in the audio (score ${score})`);
        // metadata only: this speech was not addressed to Bobby and was never transcribed
        brain.journal({ at: new Date().toISOString(), who: member.displayName, id: member.id, outcome: 'skipped', wakeScore: score, seconds });
      }
      if (text) await brain.hear({ text, lang, score, member, guild, tEnd, sttMs: ms, io });
    } catch (e) {
      console.error(`[voice] ${e.cause?.code === 'ECONNREFUSED' ? `voice service not reachable at ${VOICE_URL} (is it running?)` : e.message}`);
    } finally {
      busy.delete(userId);
    }
  });
}

client.once(Events.ClientReady, async () => {
  console.log(`Bobby online: ${client.user.tag}`);
  console.log(`[rights] role needed per action: ${Object.entries(brain.roleTable()).map(([a, r]) => `${a}=${r}`).join(' ')}`);
  claude.check();
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
    music.stop();
    getVoiceConnection(it.guildId)?.destroy();
    return it.reply('À plus.');
  }
  if (it.commandName === 'surnoms') {
    const list = (await aliases.listText({ memberName: memberName(it.guild) })) || "Personne n'a de surnom.";
    return it.reply(list.slice(0, 2000)); // Discord message limit
  }
  const channel = it.member.voice.channel;
  if (!channel) return it.reply({ content: 'Rejoins un salon vocal d\'abord.', ephemeral: true });
  getVoiceConnection(it.guildId)?.destroy(); // a second /join must not attach a second listener (double actions)
  session = { guild: it.guild, joinerId: it.user.id };
  clearTimeout(leaveTimer); leaveTimer = null; autoPaused = false;
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
  listen(connection, channel.guild, makeIo(channel.guild, text));
}

client.login(process.env.DISCORD_TOKEN);
