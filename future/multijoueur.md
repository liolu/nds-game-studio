# Multijoueur (à faire)

Jouer à plusieurs à un jeu du studio, comme dans SpaceSpore : réseau local tout seul, et Internet
avec un code d'invitation. Aucun service externe (pas de Steam, pas de serveur à louer).

Sur la DS, le multijoueur passait par le Wi-Fi local (jusqu'à 16 consoles, « DS Download Play »)
ou la Nintendo Wi-Fi Connection. Ici, chaque joueur a le studio sur son PC : il n'y a donc rien à
télécharger, il suffit d'avoir la même version du studio et le même jeu.

## Ce qu'on reprend de SpaceSpore

Le code est dans `D:\... logiciel\spacespore\src\net.rs` (réseau), `net_ui.rs` (panneau F2,
chat) et `chat_cmd.rs` (commandes du chat).

| SpaceSpore | Ce qu'on garde |
| --- | --- |
| UDP direct, port fixe (27777) | Pareil, avec un autre port (par exemple 27778) pour pouvoir lancer les deux |
| Rien n'est ouvert tant que le joueur n'ouvre pas le panneau Multijoueur (F2) | Pareil : le réseau ne s'ouvre que si le joueur le demande |
| Réseau local : broadcast `Discover` / `HostInfo`, les PC se trouvent et se regroupent tout seuls (`pick_lan_host` : la partie la plus remplie gagne) | Pareil, mais seulement entre joueurs du même jeu (`id` de game.toml) |
| Internet : ouverture automatique du port de la box (UPnP, crate `igd-next`), puis STUN si UPnP échoue, et détection du CGNAT | Pareil |
| Code d'invitation de 7 caractères (`encode_invite` / `decode_invite` : IP + port en base 32 sans lettres ambiguës, ex. `ABC-DEFG`) | Pareil, la fonction se copie telle quelle |
| L'hôte relaie l'état de tout le monde (`Snapshot`), les clients envoient le leur (`State`) | Pareil pour les jeux en « état partagé » (voir plus bas) |
| `Hello` vérifie le protocole, la version du jeu et l'empreinte du monde : sinon `Reject` avec une raison lisible | Pareil : version du studio + `id` du jeu + version du jeu |
| Profil (pseudo, couleur) envoyé seulement quand il change, gardé en cache sur disque | Pareil : pseudo + couleur, réglés une fois dans le studio |
| Chat (Entrée pour écrire), messages confirmés par l'hôte, historique de 50 messages | Pareil, affiché sur l'écran du bas |
| 16 joueurs maximum, paquet toutes les 50 ms, coupure après 6 s sans nouvelles | 16 comme la DS ; cadence réglable par le jeu |

Ce qui ne sert pas ici : guildes, diplomatie, étoiles revendiquées, voxels, modèles partagés,
horloge d'univers.

## Ce qui change pour un moteur de jeux DS

SpaceSpore n'a qu'un seul jeu. Le studio en a plusieurs, très différents : le moteur fournit
donc le transport, et chaque jeu choisit comment se synchroniser dans son `game.toml`.

```toml
[multiplayer]
players = 4          # joueurs maximum (2 à 16)
mode = "inputs"      # inputs | state
local = true         # réseau local
online = true        # Internet avec code d'invitation
```

Deux façons de se synchroniser :

- **`inputs` (entrées synchronisées)** : chaque PC envoie seulement ses boutons DS et son stylet
  à chaque pas de `FixedUpdate`, et tous les PC font tourner exactement la même partie. Très peu
  de données. C'est bien pour les jeux de combat, de course et de réflexion à 2-4 joueurs. Le jeu
  doit être déterministe : même graine aléatoire, pas de `Time` réel dans la logique. On peut y
  ajouter un rollback plus tard (crate `ggrs`).
- **`state` (état partagé, comme SpaceSpore)** : chaque joueur envoie l'état de ce qu'il contrôle
  (position, animation...) et l'hôte relaie tout. Ça tolère mieux un réseau lent, et le jeu n'a
  pas besoin d'être déterministe. C'est bien pour les jeux d'exploration, les jeux de plateforme
  en coopération et les jeux à beaucoup de joueurs.

Ce que le moteur donnerait aux jeux :

- une ressource `DsNet` : état de la session (hors ligne, hôte, en train de rejoindre, connecté),
  joueurs connectés (numéro, pseudo, couleur), numéro du joueur local ;
- en mode `inputs` : `DsInput` et `Stylus` par joueur (`PlayerInputs[n]`) au lieu d'un seul ;
- en mode `state` : envoyer et recevoir l'état d'un composant marqué `Networked` (le jeu dit quoi
  envoyer avec serde) ;
- des messages pour les événements rares (« pièce ramassée », « partie terminée ») ;
- la condition `playing("id")` reste la même, avec en plus `is_host()` pour ce que seul l'hôte décide.

## Interface

- **F2** ou une entrée « Multijoueur » dans le menu du studio : pseudo, couleur, joueurs
  connectés, « Mon code : ABC-DEFG » (bouton copier), champ « Code d'un ami ».
- Réseau local : rien à faire, les joueurs du même jeu se retrouvent tout seuls.
- L'écran tactile sert de salle d'attente (liste des joueurs, bouton « Prêt ») avant de lancer la
  partie, puis affiche le chat pendant la partie.
- Messages d'erreur lisibles, repris de SpaceSpore : box sans UPnP, connexion partagée (CGNAT),
  version différente, partie pleine.

## Étapes

1. Transport : copier depuis SpaceSpore le socket UDP, `Msg` (avec `serde_json` pour commencer),
   `Hello` / `Welcome` / `Reject`, la découverte en réseau local, `encode_invite` /
   `decode_invite`, UPnP et STUN. Mettre tout ça dans `crates/engine/src/net/`.
2. Panneau F2 + salle d'attente sur l'écran du bas + chat.
3. Mode `state` (le plus proche de SpaceSpore), avec une démo : le jeu de plateforme en coopération,
   chaque joueur voit les autres.
4. Mode `inputs` avec une démo : le jeu de bulles en duel, chacun sur sa moitié de l'écran.
5. Plus tard : rollback (`ggrs`), observateurs, et un format binaire plus compact que JSON.

## Points d'attention

- Positions en pixels DS : elles sont plus petites que dans SpaceSpore (pas d'origine flottante
  ni de f64), donc les paquets sont légers.
- Un jeu en mode `inputs` ne doit utiliser que `FixedUpdate` et son propre générateur aléatoire
  avec une graine (comme `Round::random` dans le jeu de bulles).
- Les tests de SpaceSpore (`net.rs`, `#[cfg(test)]`) qui simulent plusieurs joueurs dans un même
  processus sont à reprendre : ils évitent de lancer deux PC pour tester.
- Pare-feu Windows : la première ouverture du port fait apparaître la fenêtre d'autorisation de
  Windows. L'expliquer dans le panneau F2.
