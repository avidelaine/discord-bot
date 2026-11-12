// Fake server shared by the tests: Alex speaks from Général.
const m = (name, channel, id = name, nicknames = []) => ({ id, name, channel, nicknames, aliases: nicknames });
module.exports = {
  speaker: m('Alex', 'Général'),
  members: [
    m('Alex', 'Général'),
    m('Zyklon', 'Général', 'Zyklon', ['le Grec']),
    m('Kévin', 'Général'),
    m('Marie', 'Gaming'),
    m('Lucas', 'Gaming', 'lucas'),
    m('Prime Ilias', 'Gaming', '397478613559148545'),
    m('Sorciax', 'Gaming', 'Sorciax', ['Vinice Lapute']),
    m('WakandeX', 'Gaming', '261531632589078528'),
  ],
  voiceChannels: ['Général', 'Gaming', 'AFK', 'Réunion'].map((name) => ({ name })),
  textChannels: ['général', 'annonces', 'memes', 'dev-bot-test'].map((name) => ({ name })),
};
module.exports.speaker = module.exports.members[0];
