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

const VOICE_URL = process.env.VOICE_URL || 'http://127.0.0.1:3000';
// Silence that ends a sentence. Shorter is snappier, but cuts you off if you pause mid-sentence.
const SILENCE_MS = parseInt(process.env.RECORD_SILENCE_MS || '600');
// Text channel (name or ID) where Bobby writes what it says. Empty: the channel where /join was typed.
const TEXT_CHANNEL = process.env.TEXT_CHANNEL || '';

const client = new Client({ intents: [GatewayIntentBits.Guilds, GatewayIntentBits.GuildVoiceStates] });
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

function makeIo(guild, textChannel) {
  return {
    music,
    beep: () => { mixer.overlay(new PassThrough().end(BEEP)); },
    async say(text, lang = 'fr') {
      console.log(`[bobby] ${text}`);
      textChannel.send(`🗣️ ${text}`).catch(() => {});
      const res = await fetch(`${VOICE_URL}/tts?lang=${lang}&text=${encodeURIComponent(text)}`);
      mixer.overlay(decode(Readable.fromWeb(res.body)));
    },
    leave: () => getVoiceConnection(guild.id)?.destroy(),
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
  await client.application.commands.set([]); // drop global commands left by older versions: only the server ones below remain
  await client.guilds.cache.get(process.env.GUILD_ID)?.commands.set([
    { name: 'join', description: 'Bobby rejoint ton salon vocal' },
    { name: 'leave', description: 'Bobby quitte le vocal' },
  ]);
});

client.on(Events.InteractionCreate, async (it) => {
  if (!it.isChatInputCommand()) return;
  if (it.commandName === 'leave') {
    getVoiceConnection(it.guildId)?.destroy();
    return it.reply('À plus.');
  }
  const channel = it.member.voice.channel;
  if (!channel) return it.reply({ content: 'Rejoins un salon vocal d\'abord.', ephemeral: true });
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
});

client.login(process.env.DISCORD_TOKEN);
