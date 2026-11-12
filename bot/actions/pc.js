// PC control, super admin only: media + Windows volume (via voice/pc.py), launching games and apps,
// lock / sleep / shutdown (confirmed first), GPU status.
// BOBBY_PC_DRY=1 (used by the tests): nothing is spawned or changed, calls are only recorded in `calls`.
// No simulated keyboard or mouse input anywhere: Riot Vanguard would not like it.
const fs = require('fs');
const path = require('path');
const { spawn, execFile, execFileSync } = require('child_process');
const { sim } = require('../util');

const DRY = () => process.env.BOBBY_PC_DRY === '1';
const calls = [];
const VOICE = () => process.env.VOICE_URL || 'http://127.0.0.1:3000';

// Every side effect goes through here. Hidden, detached, no input.
function sh(file, args) {
  calls.push([file, ...args]);
  if (DRY()) return console.log(`[pc dry] ${file} ${args.join(' ')}`);
  spawn(file, args, { detached: true, stdio: 'ignore', windowsHide: true }).on('error', () => {}).unref();
}
const open = (target) => sh('explorer.exe', [target]); // opens .lnk, .url and steam:// style links through their handler

async function voice(p, method = 'GET') {
  if (DRY() && method === 'POST') { calls.push(['POST', p]); return {}; }
  const r = await fetch(`${VOICE()}${p}`, { method, signal: AbortSignal.timeout(5000) });
  if (!r.ok) throw new Error(`voice ${r.status}`);
  return r.json();
}

// ---------- Index of launchable games and apps (built once, on first use) ----------

const walk = (dir) => { try { return fs.readdirSync(dir, { withFileTypes: true }).flatMap((e) => (e.isDirectory() ? walk(path.join(dir, e.name)) : [path.join(dir, e.name)])); } catch { return []; } };
const read = (f) => { try { return fs.readFileSync(f, 'utf8'); } catch { return ''; } };

function steamGames() {
  const root = process.env.STEAM_DIR || 'C:\\Program Files (x86)\\Steam';
  const libs = [root, ...[...read(path.join(root, 'steamapps', 'libraryfolders.vdf')).matchAll(/"path"\s+"([^"]+)"/g)].map((m) => m[1].replace(/\\\\/g, '\\'))];
  // manifests sit directly in steamapps/: never recurse into the game files
  const manifests = (l) => { try { return fs.readdirSync(path.join(l, 'steamapps')).filter((f) => /^appmanifest_\d+\.acf$/.test(f)).map((f) => path.join(l, 'steamapps', f)); } catch { return []; } };
  return [...new Set(libs)].flatMap((l) => manifests(l).map((f) => {
    const t = read(f);
    return { name: t.match(/"name"\s+"([^"]+)"/)?.[1], id: t.match(/"appid"\s+"(\d+)"/)?.[1] };
  })).filter((g) => g.name && g.id && !/Steamworks|Proton|Steam Linux/i.test(g.name))
    .map((g) => ({ name: g.name, kind: 'steam', open: () => open(`steam://rungameid/${g.id}`) }));
}

const epicGames = () => walk('C:\\ProgramData\\Epic\\EpicGamesLauncher\\Data\\Manifests').filter((f) => f.endsWith('.item')).flatMap((f) => {
  try {
    const j = JSON.parse(read(f));
    return j.DisplayName && j.AppName ? [{ name: j.DisplayName, kind: 'epic', open: () => open(`com.epicgames.launcher://apps/${encodeURIComponent(j.AppName)}?action=launch&silent=true`) }] : [];
  } catch { return []; }
});

function riotGames() {
  let exe = 'C:\\Riot Games\\Riot Client\\RiotClientServices.exe';
  try { const j = JSON.parse(read('C:\\ProgramData\\Riot Games\\RiotClientInstalls.json')); exe = (j.rc_live || j.rc_default || exe).replace(/\//g, '\\'); } catch {}
  return fs.existsSync(exe) ? [{ name: 'Valorant', kind: 'riot', open: () => sh(exe, ['--launch-product=valorant', '--launch-patchline=live']) }] : [];
}

const startMenu = () => [process.env.ProgramData, process.env.APPDATA].filter(Boolean)
  .flatMap((d) => walk(path.join(d, 'Microsoft', 'Windows', 'Start Menu', 'Programs')))
  .filter((f) => /\.(lnk|url)$/i.test(f) && !/uninstall|désinstall|readme|documentation|\bhelp\b/i.test(path.basename(f)))
  .map((f) => ({ name: path.basename(f).replace(/\.(lnk|url)$/i, ''), kind: 'app', open: () => open(f) }));

// Store apps (Claude, ChatGPT...) have no .lnk: Get-StartApps lists them, explorer opens them by AppID.
function storeApps() {
  try {
    const out = execFileSync('powershell.exe', ['-NoProfile', '-Command', 'Get-StartApps | ForEach-Object { $_.Name + "|" + $_.AppID }'], { windowsHide: true, timeout: 15000, encoding: 'utf8' });
    return out.split(/\r?\n/).map((l) => l.split('|')).filter(([n, id]) => n && id?.includes('!'))
      .map(([name, id]) => ({ name, kind: 'store', open: () => open(`shell:AppsFolder\\${id.trim()}`) }));
  } catch { return []; }
}

let INDEX; // ponytail: built once per run; restart Bobby after installing a game
const index = () => (INDEX ||= [...riotGames(), ...steamGames(), ...epicGames(), ...startMenu(), ...storeApps()]);

// Best entry for a spoken name; games come first so they win ties, then shorter names.
const ART = /^(?:le |la |les |l'|the |un |une )+|^(?:jeu|game|app|application)\s+/;
function find(q) {
  const name = q.toLowerCase().replace(ART, '').replace(ART, '');
  return index().map((e) => ({ e, s: sim(name, e.name) })).filter((x) => x.s >= 0.75)
    .sort((a, b) => b.s - a.s || a.e.name.length - b.e.name.length)[0]?.e;
}

// Windows that are open right now: [{ pid, name, title, product }]. Bobby's own parts are never listed.
const SELF = /^(?:node|python|pythonw|llama-server|explorer|powershell|pwsh|cmd|conhost|windowsterminal|dwm|svchost|csrss|winlogon)$/i;
function windows() {
  const out = execFileSync('powershell.exe', ['-NoProfile', '-Command', 'Get-Process | Where-Object { $_.MainWindowHandle -ne 0 } | ForEach-Object { "$($_.Id)`t$($_.ProcessName)`t$($_.MainWindowTitle)`t$($_.Product)" }'], { windowsHide: true, timeout: 15000, encoding: 'utf8' });
  // tab-separated: window titles often contain "|" ("Video | YouTube")
  return out.split(/\r?\n/).map((l) => l.split('\t')).filter(([pid, name]) => pid && name && !SELF.test(name)).map(([pid, name, title, product]) => ({ pid, name, title, product }));
}
// Best open window for a spoken name ("Apex Legends" matches the window title, "r5apex" the process).
function openWindow(q) {
  const name = q.toLowerCase().replace(ART, '').replace(ART, '');
  return windows().map((w) => ({ w, s: Math.max(...[w.title, w.name, w.product].filter(Boolean).map((c) => sim(name, c))) }))
    .filter((x) => x.s >= 0.75).sort((a, b) => b.s - a.s)[0]?.w;
}

const gpu = () => new Promise((ok, ko) => execFile('nvidia-smi', ['--query-gpu=name,temperature.gpu,utilization.gpu,memory.used,memory.total', '--format=csv,noheader'], { windowsHide: true, timeout: 5000 }, (e, out) => {
  if (e) return ko(e);
  const [name, temp, load, used, total] = out.trim().split(/,\s*/);
  const gb = (v) => (parseInt(v, 10) / 1024).toFixed(1);
  ok({ name: name.replace(/NVIDIA GeForce /, ''), temp, load: parseInt(load, 10), used: gb(used), total: gb(total) });
}));

// A step that may fail marks s.fail instead of throwing, so Bobby says it out loud.
const safe = (fn) => async (s) => { try { await fn(s); } catch (e) { console.error(`[pc] ${e.message}`); s.fail = true; } };
const OFF = { fr: "Je n'arrive pas à joindre le service du PC.", en: "I can't reach the PC service." };
const say = (fr, en) => ({ fr: (s) => (s.fail ? OFF.fr : fr(s)), en: (s) => (s.fail ? OFF.en : en(s)) });

const MEDIA = {
  play: ['Je relance la musique.', 'Resuming.'],
  pause: ['Je mets en pause.', 'Paused.'],
  next: ['Musique suivante.', 'Next track.'],
  previous: ['Musique précédente.', 'Previous track.'],
};

// "on the PC" / "sur Spotify": explicit target, so Bobby's own Discord player can keep "pause", "skip", "volume".
const ON = '(?: (?:sur|on) (?:le pc|the pc|spotify|youtube music))';
const VOL = '(?:(?:le )?(?:son|volume) (?:du pc|de windows|windows|sur le pc)|the (?:pc|windows) (?:volume|sound))';
const act = (type, text = '') => [{ type, text }];

module.exports = {
  help: {
    fr: 'piloter le PC (musique, volume, lancer et fermer des jeux et applications, verrouiller, veille, éteindre)',
    en: 'control the PC (music, volume, launch and close games and apps, lock, sleep, shut down)',
  },
  short: { fr: 'le PC', en: 'the PC' },
  prompt: [
    '- pc_media : contrôler la musique du PC (Spotify, YouTube Music, navigateur), "text" vaut play, pause, next ou previous',
    '- pc_music : dire quelle musique joue sur le PC',
    '- pc_volume : volume de Windows (le PC, pas Discord), "text" vaut up, down, mute, unmute ou un nombre de 0 à 100',
    '- pc_launch : lancer le jeu ou l\'application nommé dans "text" (Valorant, Steam, Epic, Claude, ChatGPT...)',
    '- pc_close : fermer le jeu ou l\'application nommé dans "text" ("ferme Apex", "quitte Spotify")',
    '- pc_lock / pc_sleep / pc_shutdown : verrouiller, mettre en veille ou éteindre le PC',
    '- pc_gpu : température, charge et mémoire de la carte graphique',
  ],
  rules: [
    '- pc_media et pc_volume seulement si on parle du PC, de Windows, de Spotify ou de YouTube Music. "lance" ou "ouvre" suivi d\'un jeu ou d\'une application, c\'est pc_launch. "ferme", "quitte" ou "close" suivi d\'un jeu ou d\'une application, c\'est pc_close, jamais pc_launch.',
  ],
  examples: [
    '"mets la musique en pause sur le PC" -> {"actions":[{"type":"pc_media","text":"pause"}],"say":""}',
    '"passe à la musique suivante sur Spotify" -> {"actions":[{"type":"pc_media","text":"next"}],"say":""}',
    '"baisse le son du PC à 30" -> {"actions":[{"type":"pc_volume","text":"30"}],"say":""}',
    '"ouvre ChatGPT" -> {"actions":[{"type":"pc_launch","text":"ChatGPT"}],"say":""}',
    '"launch Valorant" -> {"actions":[{"type":"pc_launch","text":"Valorant"}],"say":""}',
    '"ferme Apex Legends" -> {"actions":[{"type":"pc_close","text":"Apex Legends"}],"say":""}',
    '"mets le PC en veille" -> {"actions":[{"type":"pc_sleep"}],"say":""}',
    '"what song is playing on the PC" -> {"actions":[{"type":"pc_music"}],"say":""}',
    '"mute the PC volume" -> {"actions":[{"type":"pc_volume","text":"mute"}],"say":""}',
  ],
  quick: [
    [new RegExp(`^(?:mets? (?:la musique )?en pause|mets? pause|pause(?: la musique| the music)?)${ON}$`), () => act('pc_media', 'pause')],
    [new RegExp(`^(?:(?:reprends|relance|remets) la musique|resume (?:the )?music)${ON}$`), () => act('pc_media', 'play')],
    [new RegExp(`^(?:(?:musique|morceau|titre|chanson) suivante?|prochaine (?:musique|chanson))${ON}$|^next (?:song|track)${ON}$`), () => act('pc_media', 'next')],
    [new RegExp(`^(?:(?:musique|morceau|titre|chanson) précédente?)${ON}$|^(?:previous|last) (?:song|track)${ON}$`), () => act('pc_media', 'previous')],
    [/^(?:c'est quoi|quelle est|quel est|qu'est-ce qui (?:joue|passe)) (?:la |le )?(?:musique|morceau|titre|chanson)(?: qui (?:joue|passe))? (?:sur|du) (?:le )?pc$|^what(?:'s| is) (?:the )?(?:music |song |track )?playing (?:on|from) (?:the )?pc$|^what(?:'s| is) (?:the )?(?:music|song|track) (?:on|from) (?:the )?pc$/, () => act('pc_music')],
    [new RegExp(`^(monte|augmente|baisse|diminue|descends|turn up|raise|increase|turn down|lower|decrease|reduce) ${VOL}$`), (m) => act('pc_volume', /^(?:monte|augmente|turn up|raise|increase)$/.test(m[1]) ? 'up' : 'down')],
    [new RegExp(`^(?:mets|met|règle|regle) ${VOL} (?:à|a|sur) (\\d+)(?: ?%| pour cent| percent)?$|^set ${VOL} to (\\d+)(?: ?%| percent)?$`), (m) => act('pc_volume', m[1] || m[2])],
    [new RegExp(`^(?:coupe|mute|mets en sourdine) ${VOL}$`), () => act('pc_volume', 'mute')],
    [new RegExp(`^(?:remets|rétablis|réactive|redonne|unmute|démute) ${VOL}$`), () => act('pc_volume', 'unmute')],
    // Only taken when the name is really a game or app on this PC, so "lance de la musique" and the like go elsewhere.
    [/^(?:lance|ouvre|démarre|demarre|launch|open|start) (?!(?:de la |la |une? |some |the )?(?:musique|chanson|music|song|playlist|morceau))(.+)$/, (m) => (find(m[1]) ? act('pc_launch', m[1]) : null)],
    [/^(?:ferme|quitte|close|quit|kill) (?!(?:le |la |the )?(?:salon|vocal|channel|voice|musique|music|radio|micro|mic|son)\b)(.+)$/, (m) => act('pc_close', m[1])],
    [/^(?:verrouille|bloque) (?:le pc|l'ordi(?:nateur)?|mon pc|la session)$|^lock (?:the |my )?pc$/, () => act('pc_lock')],
    [/^mets? (?:le pc|l'ordi(?:nateur)?|mon pc) en (?:veille|sommeil)$|^(?:put|set) (?:the |my )?pc (?:to sleep|in sleep mode)$/, () => act('pc_sleep')],
    [/^(?:éteins|eteins|arrête|arrete|shut ?down|turn off|shut down) (?:le pc|l'ordi(?:nateur)?|mon pc|the pc|my pc)$/, () => act('pc_shutdown')],
    [/^(?:c'est quoi |quelle est )?(?:la |l')?(?:température|temperature|charge|état|etat) (?:du|de la) (?:gpu|carte graphique)$|^(?:comment va|how'?s) (?:le|la|the) (?:gpu|carte graphique)$|^(?:gpu|graphics card) (?:status|temp(?:erature)?|usage)$|^how hot is (?:the )?gpu$/, () => act('pc_gpu')],
  ],
  actions: {
    pc_media: {
      role: 'super', text: true,
      run: safe(async (s) => { if (!MEDIA[s.text]) throw new Error(`unknown media command "${s.text}"`); await voice(`/pc/media/${s.text}`, 'POST'); }),
      say: say((s) => MEDIA[s.text][0], (s) => MEDIA[s.text][1]),
    },
    pc_music: {
      role: 'super',
      run: safe(async (s) => { s.res = await voice('/pc/media'); }),
      say: say(
        (s) => (s.res.title ? `${s.res.playing ? 'Sur le PC' : 'En pause sur le PC'} : ${s.res.title}${s.res.artist ? `, de ${s.res.artist}` : ''}.` : 'Rien ne joue sur le PC.'),
        (s) => (s.res.title ? `${s.res.playing ? 'On the PC' : 'Paused on the PC'}: ${s.res.title}${s.res.artist ? ` by ${s.res.artist}` : ''}.` : 'Nothing is playing on the PC.'),
      ),
    },
    pc_volume: {
      role: 'super', text: true,
      run: safe(async (s) => {
        const t = s.text.toLowerCase().trim();
        const q = /^\d+$/.test(t) ? `level=${Math.min(100, +t)}` : { up: 'delta=10', down: 'delta=-10', mute: 'mute=true', unmute: 'mute=false' }[t];
        if (!q) throw new Error(`unknown volume "${s.text}"`);
        s.res = await voice(`/pc/volume?${q}`, 'POST');
      }),
      say: say(
        (s) => (s.res.muted ? 'Le PC est coupé.' : `Volume du PC à ${s.res.level ?? s.text}.`),
        (s) => (s.res.muted ? 'The PC is muted.' : `PC volume at ${s.res.level ?? s.text}.`),
      ),
    },
    pc_launch: {
      role: 'super', text: true,
      run(s) { const e = find(s.text); if (e) { s.found = e.name; e.open(); } },
      say: {
        fr: (s) => (s.found ? `Je lance ${s.found}.` : `Je ne trouve pas « ${s.text} » sur le PC.`),
        en: (s) => (s.found ? `Launching ${s.found}.` : `I can't find "${s.text}" on the PC.`),
      },
    },
    pc_close: {
      role: 'super', text: true,
      run: safe((s) => {
        if (DRY()) { s.found = s.text; return sh('taskkill.exe', ['/IM', s.text, '/T', '/F']); }
        const w = openWindow(s.text);
        if (w) { s.found = w.title || w.name; sh('taskkill.exe', ['/PID', w.pid, '/T', '/F']); }
      }),
      say: say((s) => (s.found ? `Je ferme ${s.found}.` : `Je ne trouve pas « ${s.text} » ouvert sur le PC.`), (s) => (s.found ? `Closing ${s.found}.` : `I can't find "${s.text}" open on the PC.`)),
    },
    pc_lock: { role: 'super', confirm: true, run: () => sh('rundll32.exe', ['user32.dll,LockWorkStation']), say: { fr: () => 'Je verrouille le PC.', en: () => 'Locking the PC.' } },
    // ponytail: SetSuspendState gives real sleep (rundll32 powrprof would hibernate if hibernation is enabled).
    pc_sleep: {
      role: 'super', confirm: true,
      run: () => sh('powershell.exe', ['-NoProfile', '-Command', "Add-Type -AssemblyName System.Windows.Forms; [System.Windows.Forms.Application]::SetSuspendState('Suspend', $false, $false)"]),
      say: { fr: () => 'Je mets le PC en veille.', en: () => 'Putting the PC to sleep.' },
    },
    pc_shutdown: { role: 'super', confirm: true, run: () => sh('shutdown.exe', ['/s', '/t', '5']), say: { fr: () => "J'éteins le PC.", en: () => 'Shutting down the PC.' } },
    pc_gpu: {
      role: 'super',
      run: safe(async (s) => { s.res = await gpu(); }),
      say: {
        fr: (s) => (s.fail ? 'Je ne peux pas lire la carte graphique.' : `${s.res.name} : ${s.res.temp} degrés, ${s.res.load} % de charge, ${s.res.used} Go de mémoire sur ${s.res.total}.`),
        en: (s) => (s.fail ? "I can't read the graphics card." : `${s.res.name}: ${s.res.temp} degrees, ${s.res.load}% load, ${s.res.used} of ${s.res.total} GB memory.`),
      },
    },
  },
  warm: () => setTimeout(index, 1000), // build the game/app index at startup, not in the middle of a sentence
  _test: { calls, index, find },
};
