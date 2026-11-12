// Simple questions answered locally, open to Prime: the time, and what Bobby can do.
const fs = require('fs');

// Each action module describes itself in "help"; the list follows whatever modules exist.
const capabilities = (lang) => fs.readdirSync(__dirname).filter((f) => f.endsWith('.js'))
  .map((f) => require(`./${f}`).help?.[lang]).filter(Boolean).join(', ');

const now = (lang) => new Date().toLocaleString(lang === 'en' ? 'en-GB' : 'fr-FR', { weekday: 'long', day: 'numeric', month: 'long', hour: '2-digit', minute: '2-digit' });

module.exports = {
  help: { fr: "dire l'heure", en: 'tell the time' },
  prompt: [
    '- time : dire la date et l\'heure',
    '- help : dire ce que tu sais faire (lister tes fonctionnalités)',
  ],
  rules: [],
  examples: [
    '"il est quelle heure" -> {"actions":[{"type":"time"}],"say":""}',
    '"qu\'est-ce que tu sais faire" -> {"actions":[{"type":"help"}],"say":""}',
    '"liste-moi tes fonctionnalités" -> {"actions":[{"type":"help"}],"say":""}',
    '"how are you doing" -> {"actions":[],"say":"I\'m doing great, thanks!"}',
    '"t\'es qui toi" -> {"actions":[],"say":"Je suis Bobby, l\'assistant vocal du serveur."}',
  ],
  quick: [
    [/^(?:quelle heure (?:est-il|il est|est il)|il est quelle heure|what time is it|what's the time)$/, () => [{ type: 'time' }]],
    [/\blist\w* (?:moi )?(?:tes |les |your )?(?:features|fonctionnalit|commandes|capacit|commands)|qu'est-ce que tu sais faire|que sais-tu faire|tu sais faire quoi|what can you do/, () => [{ type: 'help' }]],
  ],
  actions: {
    time: {
      role: 'prime',
      run() {},
      say: { fr: () => `Nous sommes ${now('fr')}.`, en: () => `It's ${now('en')}.` },
    },
    help: {
      role: 'prime',
      run() {},
      say: { fr: () => `Je sais ${capabilities('fr')}.`, en: () => `I can ${capabilities('en')}.` },
    },
  },
  capabilities,
};
