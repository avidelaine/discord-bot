// Comprehension test set: sentence -> expected actions, against the real LLM (llama-server running).
// Run: node bot/intent.test.js   Add every sentence Bobby gets wrong in real use.
// Shortcuts, names and rights are covered without the LLM in unit.test.js.
const assert = require('assert');
const { understand, resolve } = require('./brain');

const ctx = require('./test-context');

// [sentence, expected] ; expected = "type:targets>channel" per action, joined by " | ". [] = no action.
const CASES = [
  ['coupe le micro de Kévin', 'mute:Kévin'],
  ['mute ziklone', 'mute:Zyklon'],
  ['rends la parole à Kevin', 'unmute:Kévin'],
  ['coupe le son de Marie', 'deafen:Marie'],
  ['Marie peut réentendre', 'undeafen:Marie'],
  ['déplace Zyklon dans gaming', 'move:Zyklon>Gaming'],
  ['envoie Lucas en AFK', 'move:Lucas>AFK'],
  ['mets-moi dans la salle de réunion', 'move:Alex>Réunion'],
  ['ramène Marie ici', 'move:Marie>Général'],
  ['tout le monde dans gaming', 'move:Alex,Kévin,Lucas,Marie,Prime Ilias,Sorciax,WakandeX,Zyklon>Gaming'],
  ['coupe tout le monde sauf Kévin', 'mute:Zyklon'],
  ['vire Lucas du vocal', 'disconnect:Lucas'],
  ['déplace Kévin dans AFK et coupe son micro', 'move:Kévin>AFK | mute:Kévin'],
  ['écris dans annonces que la soirée commence à 21 heures', 'message:>annonces'],
  ['ajoute acheter du pain à ma liste', 'todo_add:'],
  ["qu'est-ce que j'ai sur ma liste", 'todo_list:'],
  ['enlève le pain de ma liste', 'todo_remove:'],
  ['annule', 'undo:'],
  ['laisse tomber, reviens en arrière', 'undo:'],
  ['tu peux partir', 'leave:'],
  ['quelle heure est-il', 'time:'],
  ['raconte-moi une blague', 'ask:'],
  ['déplace Bernard dans gaming', 'ERR'],
  ['mute Zyklon et Kévin', 'mute:Kévin,Zyklon'],
  ['euh coupe le micro de... de Marie', 'mute:Marie'],
  // Music (Bobby's own player)
  ['mets du hard rock', 'music_play:'],
  ['est-ce que tu peux jouer Alors on danse de Stromae', 'music_play:'],
  ['ajoute Blinding Lights à la file', 'music_add:'],
  ['la chanson suivante', 'music_skip:'],
  ['mets le volume un peu moins fort à 30', 'music_volume:'],
  ['play Around the World by Daft Punk', 'music_play:'],
  ['put on some chill lofi', 'music_play:'],
  ['add Starboy to the queue', 'music_add:'],
  ['what song is playing right now', 'music_now:'],
  ['turn the music down', 'music_volume:'],
  ['pause the music on Spotify', 'pc_media:'],
  // English
  ['mute Kevin', 'mute:Kévin'],
  ['give Zyklon his mic back', 'unmute:Zyklon'],
  ['deafen Marie please', 'deafen:Marie'],
  ['move Lucas to the AFK channel', 'move:Lucas>AFK'],
  ['bring Marie over here', 'move:Marie>Général'],
  ['take me to gaming', 'move:Alex>Gaming'],
  ['mute everyone except Kevin', 'mute:Zyklon'],
  ['kick Lucas out of voice', 'disconnect:Lucas'],
  ['post in announcements that the game starts at nine', 'message:>annonces'],
  ['add buy milk to my list', 'todo_add:'],
  ['undo that', 'undo:'],
  ['you can go now', 'leave:'],
  ['how are you doing', []],
  // Nicknames (Zyklon is "le Grec" in the test context)
  ['appelle Kévin le Boss', 'alias_add:Kévin'],
  ["Zyklon ne s'appelle plus le Grec", 'alias_remove:Zyklon'],
  ['vire le Grec du vocal', 'disconnect:Zyklon'],
  ['coupe le micro du Grec', 'mute:Zyklon'],
  ['call Lucas the rookie', 'alias_add:Lucas'],
  ['qui a quel surnom sur le serveur', 'alias_list:'],
  ['who has which nickname', 'alias_list:'],
  ['change le surnom le Grec en le Spartiate', 'alias_rename:'],
  ['efface le surnom le Grec de Zyklon', 'alias_remove:Zyklon'],
  ['remove the nickname le Grec from Zyklon', 'alias_remove:Zyklon'],
  // Questions forwarded to Claude
  ["c'est quoi la capitale de l'Australie", 'ask:'],
  ['explique-moi comment marche un trou noir', 'ask:'],
  ['combien font 17 fois 23', 'ask:'],
  ['who won the last world cup', 'ask:'],
  ['salut Bobby ça va', []],
  // Missed on 2026-11-12 (journal)
  ['il est quelle heure ?', 'time:'],
  ['est-ce que tu peux me dire la population de la République démocratique du Congo ?', 'ask:'],
  ["c'est quoi la République Démocratique du Congo ?", 'ask:'],
  ["est-ce que tu peux me lister les features qu'on t'a déjà implémentées ?", 'help:'],
  ["est-ce que tu peux kick Sorciax s'il te plaît ?", 'disconnect:Sorciax'],
  ['est-ce que tu peux renommer Lucas en le Boss ?', 'alias_add:Lucas'],
  // "des mute Ilias" is taken by the shortcut before the LLM (unit.test.js); the LLM alone reads it as deafen.
  ['peux-tu draguer Sorciax ?', 'move:Sorciax>Général'],
  ['vas-y mute moi', 'mute:Alex'],
  ['can you mute Prime Ilias please?', 'mute:Prime Ilias'],
  ["est-ce que tu peux me mute micro et casque s'il te plaît ?", 'mute:Alex | deafen:Alex'],
  ['Mute Bobby', []], // Bobby says it cannot mute itself
  // Routing: commands stay local, only knowledge questions go to Claude (ask)
  ['tu sais faire quoi', 'help:'],
  ['what time is it', 'time:'],
  ["t'es qui toi", []],
  ['mets un surnom à Marie, la Reine', 'alias_add:Marie'],
  ['écris dans memes que Lucas a perdu', 'message:>memes'],
  ['pourquoi le ciel est bleu', 'ask:'],
  ['quel temps fait-il à Paris demain', 'ask:'],
  ['how many people live in Canada', 'ask:'],
  ['donne-moi une idée de recette pour ce soir', 'ask:'],
  // PC control (super admin; the actions only run for the super role)
  ['mets la musique du PC en pause', 'pc_media:'],
  ['est-ce que tu peux passer au morceau suivant sur Spotify', 'pc_media:'],
  ['baisse le volume de Windows à 20', 'pc_volume:'],
  ['démarre le jeu Valorant', 'pc_launch:'],
  ['turn the PC volume up', 'pc_volume:'],
  ['what is the temperature of my graphics card', 'pc_gpu:'],
  ['put my computer to sleep', 'pc_sleep:'],
  // Rights by voice
  ['enlève les permissions de Lucas', 'perm_revoke:Lucas'],
  ['passe Marie admin', 'admin_add:Marie'],
];

// Adjacent steps of the same type and channel count as one ("mute Zyklon" + "mute Kévin" = "mute Zyklon and Kévin").
const merge = (plan) => plan.reduce((out, s) => {
  const last = out.at(-1);
  if (last && last.type === s.type && last.channel === s.channel && last.targets && s.targets) last.targets = [...last.targets, ...s.targets];
  else out.push({ ...s });
  return out;
}, []);
const fmt = (plan) => merge(plan).map((s) => `${s.type}:${(s.targets || []).map((t) => t.name).sort().join(',')}${s.channel ? `>${s.channel.name}` : ''}`).join(' | ');


(async () => {
  let ok = 0;
  const times = [];
  for (const [phrase, want] of CASES) {
    const t0 = performance.now();
    let got;
    try {
      const out = await understand(phrase, ctx);
      got = out.actions.length ? fmt(resolve(out.actions, ctx)) : [];
    } catch (e) {
      if (e.code === 'LLM_DOWN') throw new Error('llama-server is not answering on port 8080');
      got = 'ERR';
    }
    times.push(performance.now() - t0);
    const pass = Array.isArray(want) ? Array.isArray(got) : got === want;
    if (pass) ok++;
    else console.log(`FAIL  "${phrase}"\n   expected ${JSON.stringify(want)}\n   got      ${JSON.stringify(got)}`);
  }
  times.sort((a, b) => a - b);
  const med = Math.round(times[times.length >> 1]);
  console.log(`\nComprehension: ${ok}/${CASES.length}  ·  LLM latency median ${med} ms, max ${Math.round(times.at(-1))} ms`);
  process.exit(ok / CASES.length >= 0.9 ? 0 : 1);
})();
