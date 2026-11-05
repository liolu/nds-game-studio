# Notes de version (patch notes)

Chaque ajout ou changement s'écrit ici, dans la section « Pas encore publié », au moment où il est fait
(règle de `CLAUDE.md`). À la publication d'une version, la section prend son numéro et sa date.

## Pas encore publié

### Ajouts

- **Réglages des jeux, généraux et par jeu** : touches, vitesse, son et langue. Les réglages généraux sont dans
  Options > Jeux (l'ancienne page Clavier). Chaque jeu a les siens dans son onglet Paramètres : une valeur choisie
  pour un jeu passe avant la générale, « Comme le général » la retire, et une ligne dit d'où vient chaque valeur
  (ce jeu, général, par défaut). Tout est gardé dans `game_options.toml`.
- **Touches modifiables** : cliquer la touche d'un bouton DS, puis appuyer sur la nouvelle (Échap annule).
  Par position : pareil en AZERTY et en QWERTY. Un bouton changé pour un jeu garde les autres boutons généraux.
  Les touches s'affichent avec la lettre du clavier de l'ordinateur (A, Z, Q, W sur un AZERTY), et non plus avec
  les noms QWERTY.
- **Vitesse du jeu** : interrupteur du launcher, actif par défaut, qui ne joue qu'en jouant avec des mods
  (rien à installer, ce n'est pas un mod). En jeu : Tab passe de la vitesse normale à la vitesse rapide
  (x2 par défaut, x1,5 à x8 au choix) ; Page haut accélère, Page bas ralentit (x0,25 à x8). Les trois touches
  se changent comme les autres. L'interrupteur est aussi dans l'onglet Jouer des jeux qui ont des mods.
- **Volume** général et par jeu, de 0 (coupé) à 200 %.
- **Langue de la console** vue par le jeu, générale et par jeu : celle du jeu (comme avant), anglais,
  français, allemand, italien, espagnol, japonais. Un jeu ne propose que les langues qu'il contient.
- **Interrupteurs pour les mods** : chaque mod installé a un interrupteur Actif / Inactif.
- **Jeux à plusieurs versions ou zones** (comme les Nintendogs) : une seule carte dans la grille, avec les
  jaquettes des autres versions empilées derrière, sans changer la taille de la carte. Sur la page du jeu,
  des onglets Version et Zone passent de l'une à l'autre et changent la jaquette ; la dernière ouverte est
  celle de la grille. Sans autre version ou zone, les onglets ne s'affichent pas mais leur place reste :
  la page a la même mise en page pour tous les jeux. Le nom d'une version vient du titre de la rom, ou de son nom
  de fichier quand le titre n'en dit rien (roms japonaises).
- **Caméra libre sur les modèles 3D** (fenêtre Assets, F6) : bouton « Caméra libre » sous l'aperçu.
  ZQSD (WASD en QWERTY) pour se déplacer, A / E (Q / E en QWERTY) pour monter et descendre, Maj pour aller
  4 fois plus vite, glisser pour regarder autour, molette pour la vitesse, Échap pour revenir à l'orbite.

### Runtime natif (source commune, `TOOLCHAIN` 18)

- Le paquet des touches porte la vitesse (mot 14) et le volume (mot 15), et `--ds-lang` donne la langue de la
  console. Sans ces valeurs, le runtime fait exactement comme avant. Un jeu les reçoit quand son runtime suit
  la source commune (`runtime_sync.py`) ; avant, le launcher dit que la vitesse n'est pas encore gérée.
- À vitesse rapide, la musique garde son tempo : le son peut se décaler ou grésiller.

### Changements

- La page de la zone du bas d'un jeu garde sa position quand elle se refait (réglages changés un par un).
