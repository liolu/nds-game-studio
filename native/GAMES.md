# Jeux natifs : ce que donne chaque jeu

Relevé automatique de `native/tools/build_all.py`, sans aucune touche pressée : un essai court ne dit pas qu'un jeu est jouable.
Les modules restent sur le PC (`target/release/native`), jamais dans git.

| Jeu | Code | Overlays | Construction | Essai |
|---|---|---|---|---|
| battle-of-giants-dinosaurs | CD5E | 12 | ok | plante : instruction non traduite func_0203f8b4 @203f8c4, rien a l'ecran |
| blue-dragon-awakened-shadow | YBUD | 16 | ok | plante : instruction non traduite __sinit_ov000_021e751c @21e751c, rien a l'ecran |
| blue-dragon-plus | YYBP | 123 | ok | tourne, image 0, rien a l'ecran |
| children-of-mana | ASEE | 16 | ok | tourne, image 0, rien a l'ecran |
| club-house-games | ATDE | 363 | ok | plante : saut vers une adresse DS sans fonction : 205faad, rien a l'ecran |
| code-lyoko | AL5E | 16 | ok | tourne, image 0, rien a l'ecran |
| cooking-mama | ACCE | 0 | ok | tourne, image 0, rien a l'ecran |
| crafting-mama | BQ8E | 164 | ok | tourne, image 0, rien a l'ecran |
| dawn-of-heroes | CIEE | 12 | ok | plante : instruction non traduite func_02094588 @2094598, rien a l'ecran |
| dragon-ball-origins | CDZP | 0 | ok | tourne, image 0, rien a l'ecran |
| dragon-ball-z-attack-of-the-saiyans | BRPP | 0 | ok | tourne, image 0, rien a l'ecran |
| eyeshield-21 | AE4J | 10 | ok | plante : acces memoire invalide 1000e56c (ecriture) eip=1010ed98 esp=20fffd30 a, rien a l'ecran |
| fairy-tail-kardia | TFTJ | 241 | ok | tourne, image 0, rien a l'ecran |
| fairy-tail-madoushi | BH6J | 68 | ok | tourne, image 0, rien a l'ecran |
| farm-frenzy | BFZE | 0 | ok | tourne, image 0, rien a l'ecran |
| harry-potter-and-the-deathly-hallows-par | B7HE | 4 | ok | plante : acces memoire invalide 0 (ecriture) eip=0 esp=20fff864 arm=0, rien a l'ecran |
| harvest-moon-ds | ABCE | 0 | ok | tourne, image 1140, rien a l'ecran |
