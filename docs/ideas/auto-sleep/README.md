# Veille profonde automatique

- **Quoi :** après `[sleep] auto_after_min` minutes (30 par défaut, 0 = désactivée) sans rien à faire, Bobby passe seul en veille profonde, sans « Bonne nuit », et le log dit `[sleep] auto deep sleep after N min idle`. Rien ne l'endort : une phrase, une demande en cours ou une fenêtre de confirmation, un échange de messagerie, Bobby qui parle (Discord ou haut-parleurs du PC), une musique réellement en lecture (pas en pause).
- **Pourquoi :** rendre les ~4,4 Go de VRAM à la machine sans que personne n'ait à demander « Bobby, dors ».
- **Essayer :** mettre `auto_after_min = 1` sous `[sleep]` dans bobby.toml (ou dans Réglages, le champ affiche « désactivée » ou « après N min »), Bobby démarré en tout-Rust, attendre une à deux minutes, regarder l'icône et le tableau de bord passer à « asleep » ; le mot d'activation réveille comme avant. Arrêté, Bobby ne s'endort jamais.
- **Réglage :** entier de 0 à 1440 ; un champ vide, négatif ou trop grand est refusé (400).
- **Coût :** un minuteur d'une minute qui lit le réglage à chaque tick.
- **Limites :** précision d'une minute ; un « parle » resté bloqué (message de fin perdu) est ignoré après 2 minutes ; la phrase dite avec le « Bobby » qui réveille est perdue, comme avant ; seulement en mode tout-Rust.
