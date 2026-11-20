# Veille profonde automatique

- **Quoi :** après `[sleep] auto_after_min` minutes (30 par défaut, 0 = jamais) sans phrase, sans commande musique et sans que Bobby parle, il passe seul en veille profonde, sans « Bonne nuit », et le log dit `[sleep] auto deep sleep after N min idle`.
- **Pourquoi :** rendre les ~4,4 Go de VRAM à la machine sans que personne n'ait à demander « Bobby, dors ».
- **Essayer :** mettre `auto_after_min = 1` sous `[sleep]` dans bobby.toml (ou dans Réglages), attendre une minute, regarder l'icône de la barre et le tableau de bord passer à « asleep » ; le mot d'activation réveille comme avant.
- **Coût :** aucun, un minuteur d'une minute qui lit le réglage à chaque tick.
- **Limites :** « musique en cours » = « une commande musique a tourné il y a moins de N minutes » (une file plus longue que N minutes endort quand même Bobby, la musique Discord continue) ; la précision est d'une minute.
