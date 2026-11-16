# R0 spike: Discord voice in Rust

Question: can Bobby's Discord voice layer (receive users' voice through DAVE, play sound) move from
Node (`bot/bot.js`) to Rust? This crate is the smallest program that answers it. It is standalone
(own `[workspace]`), not the real port.

It logs in as the bot, joins the voice channel of one user, saves each sentence (cut after 600 ms
without packets, like `bot.js`) as 48 kHz stereo s16le WAV, and beeps on join and after each sentence.
Each sentence is saved twice: `NNN-songbird.wav` (songbird's own decoder, `VoiceTick`) and
`NNN-own.wav` (our per-packet Opus decode from `RtpPacket`, the way `bot.js` does it).

## Run

Stop Bobby first (control app, or `POST http://127.0.0.1:4100/api/stop`): the spike uses the same
token and would fight over the voice connection. Be in a voice channel of `GUILD_ID`.

```bash
# MSBuild cannot build libopus under the deep worktree path (260-char limit): use a short target dir.
CARGO_TARGET_DIR=C:/Users/alexa/AppData/Local/r0t cargo build --release
C:/Users/alexa/AppData/Local/r0t/release/discord-voice-spike.exe [user_id]
```

- Token, `GUILD_ID` and `SUPER_ADMINS` come from the main checkout's `.env` (override with `BOBBY_ENV`).
  The token is never printed.
- `user_id` defaults to the first super admin.
- WAVs go to `_logs/r0/` at the repo root (override with `R0_OUT`). A `[totals]` line is printed after
  each sentence and on Ctrl+C.
- Needs CMake and the MSVC build tools (libopus is built from source by `opus2`).

## Results (2026-11-16, Windows 11 gaming PC, Paris voice server)

| Step | Result |
|---|---|
| Login + join the user's channel | OK, 0.8 to 1.1 s from `join()` to connected |
| DAVE handshake | OK: protocol version 1 negotiated, MLS done by songbird + `davey` without errors |
| Receive | OK: 20/20 sentences, 1689 packets, 1479 of them DAVE-encrypted (the rest are Opus silence frames, which DAVE leaves in the clear) |
| Loss | 0 RTP sequence gaps, 0 DAVE decrypt failures, 0 Opus decode errors, 0 lost songbird ticks |
| Playback | OK: the join beep and the 20 per-sentence beeps were heard, while receiving |
| Transcription (voice service, `x-force: 1`) | 14/20 exact, 20/20 understandable; the 6 diffs are Whisper wording (`mets`→`met`, `douze`→`12`, `les`→`des`), identical on both decoders |
| CPU / memory while listening | 0.9 % of one core on average (6 % peak), 18 to 20 MB working set, 7 MB private |

Both decoders produced the same transcripts (19/20 identical strings, 20/20 same words): the garbled
DAVE receive that songbird PR #314 (closed, unmerged) and #322 (open) describe did not show up here.

DAVE drop counts come from a local instrumented copy of songbird (`songbird-drop-counters.patch`, applied
through `[patch.crates-io]` for the test only): songbird 0.6.1 drops undecryptable DAVE packets
**silently**, so without it a DAVE failure looks like silence.

**Verdict: Discord in Rust is a go.**

## Notes for the real port (R4)

- **Register event handlers before joining.** Discord sends each present user's `Speaking`
  (SSRC → user) once, at connect. Handlers added after `join()` returns miss it; packets can then not be
  attributed, and songbird drops DAVE packets of unknown SSRCs without a word. Use
  `manager.get_or_insert(guild)`, add the global events, then `manager.join(...)`. This cost the first
  test run.
- **Silent DAVE drops.** No user mapping, session not ready, or no key for a user: all are a plain
  `return` in `udp_rx`. Keep a watchdog ("speaking but no packets for N s") or upstream a counter.
- **Several speakers**: songbird keys everything by SSRC (one decoder per SSRC, `VoiceTick` holds all
  speakers each 20 ms), so it works the same for several speakers. Not tested with two people here.
- **Moving channels**: `manager.join` on an existing call moves it. Whether DAVE re-handshakes on a move
  (Discord does a new MLS group per channel) is not tested; expect a short gap and check it.
- **Speaking events**: `SpeakingStateUpdate` fires once per user, not per sentence. "Started speaking"
  should come from packets arriving, and "stopped" from 600 ms without packets, as here and in `bot.js`.
- **Slash commands, members, voice states**: serenity 0.12.5 (Dec 2025) with the `cache` feature gives
  `guild.voice_states` and members. The spike only uses the cache at `guild_create`. Slash commands were not
  exercised. serenity 0.12 is still songbird 0.6's default; `serenity-next` is the upcoming major.
- **Playback**: songbird's symphonia comes without codecs. Raw PCM needs symphonia's `pcm` feature. TTS
  and music need their own formats enabled, or must be decoded outside songbird and fed as raw PCM.
- Constant `RTCP decryption failed` warnings (about 3 every 4 s) appear once audio flows. They are harmless for
  receive (RTP is fine) but should be filtered from logs.
- Windows: libopus needs CMake + MSVC, and a short `CARGO_TARGET_DIR` from deep paths.
