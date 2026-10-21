# Code multijoueur de SpaceSpore (référence)

Copie de `D:\... logiciel\spacespore\src\` au commit d28b4d7 (2026-10-10). Ces fichiers ne sont
pas compilés ici : ils dépendent du reste de SpaceSpore (vaisseau, guildes, voxels...).

- `net.rs` : réseau (UDP, réseau local, UPnP, STUN, code d'invitation, hôte / clients, chat)
- `net_ui.rs` : panneau Multijoueur (F2) et chat
- `net_models.rs` : partage de modèles entre joueurs
- `chat_cmd.rs` : commandes du chat

La partie réutilisable est reprise et adaptée dans `../nds-net/`.
