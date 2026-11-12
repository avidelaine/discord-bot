// A shared to-do list in data/todo.json.
const { load, save } = require('../store');
const { sim } = require('../util');

const keep = () => { const prev = load('todo', []); return () => save('todo', prev); };

module.exports = {
  help: { fr: 'tenir une liste de tâches', en: 'keep a to-do list' },
  prompt: [
    '- todo_add / todo_remove : ajouter ou retirer "text" de la liste de tâches',
    '- todo_list : lire la liste de tâches',
  ],
  rules: [],
  examples: ['"retire les courses de ma liste" -> {"actions":[{"type":"todo_remove","text":"les courses"}],"say":""}'],
  quick: [],
  actions: {
    todo_add: {
      role: 'admin', text: true,
      run(s) { const undo = keep(); save('todo', [...load('todo', []), s.text]); return undo; },
      say: { fr: (s) => `J'ajoute « ${s.text} » à ta liste.`, en: (s) => `Added "${s.text}" to your list.` },
    },
    todo_remove: {
      role: 'admin', text: true,
      run(s) {
        const undo = keep();
        const l = load('todo', []);
        const i = l.findIndex((x) => sim(s.text, x) >= 0.7);
        s.missing = i < 0;
        if (s.missing) return null;
        l.splice(i, 1);
        save('todo', l);
        return undo;
      },
      say: {
        fr: (s) => (s.missing ? `Je ne trouve pas « ${s.text} » dans ta liste.` : `Je retire « ${s.text} » de ta liste.`),
        en: (s) => (s.missing ? `I can't find "${s.text}" in your list.` : `Removed "${s.text}" from your list.`),
      },
    },
    todo_list: {
      role: 'admin',
      run() {},
      say: {
        fr: () => { const l = load('todo', []); return l.length ? `Ta liste : ${l.join(', ')}.` : 'Ta liste est vide.'; },
        en: () => { const l = load('todo', []); return l.length ? `Your list: ${l.join(', ')}.` : 'Your list is empty.'; },
      },
    },
  },
};
