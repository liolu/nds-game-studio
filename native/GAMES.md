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
| heroes-of-mana | AHEP | 124 | ok | tourne, image 1080, rien a l'ecran |
| how-to-train-your-dragon | BHDP | 0 | ok | plante : instruction non traduite func_020ce780 @20ce790, rien a l'ecran |
| iron-man-2 | B2IE | 2 | ok | plante : instruction non traduite func_0216a3bc @216a3cc, rien a l'ecran |
| lego-star-wars-ii-the-original-trilogy | AL7E | 0 | ok | tourne, image 1080, affiche |
| livly-garden | BLIJ | 27 | ok | tourne, image 0, rien a l'ecran |
| luminous-arc-2 | YL2P | 4 | ok | plante : acces memoire invalide 83e589b9 (ecriture) eip=10009d80 esp=20fffa88 a, rien a l'ecran |
| madagascar | AMGP | 0 | ok | tourne, image 0, rien a l'ecran |
| mysims | AQIE | 17 | ok | tourne, image 0, rien a l'ecran |
| myst | YMYE | 0 | ok | tourne, image 0, rien a l'ecran |
| nintendogs-chihuahua-friends | AD2J | 28 | ok | tourne, image 0, rien a l'ecran |
| nintendogs-dachshund | ADGE | 30 | ok | tourne, image 0, rien a l'ecran |
| nintendogs-miniature-dachshund-friends | ADGJ | 28 | ok | tourne, image 0, rien a l'ecran |
| nintendogs-shiba-friends | AD3J | 28 | ok | tourne, image 0, rien a l'ecran |
| penguins-of-madagascar-the | VPEV | 0 | ok | plante : acces memoire invalide 42014f0 (ecriture) eip=10002e9a esp=eff50 arm=0, rien a l'ecran |
| plants-vs-zombies | BWBE | 3 | ok | tourne, image 0, rien a l'ecran |
| plants-vs-zombies-eu | BWBY | 3 | ok | plante : acces memoire invalide 13822010 (ecriture) eip=1019b282 esp=20fff758 a, rien a l'ecran |
| plants-vs-zombies-eu-2 | BWBX | 3 | ok | plante : acces memoire invalide 13822010 (ecriture) eip=1019b302 esp=20fff758 a, rien a l'ecran |
| pokemon-dash | APDE | 2 | ok | tourne, image 0, rien a l'ecran |
| pokemon-ranger | ARGE | 16 | ok | tourne, image 0, rien a l'ecran |
| powerpuff-girls-z | YPPJ | 0 | ok | tourne, image 0, rien a l'ecran |
| professor-layton-and-the-spectre-s-call | BLFP | 47 | ok | fin 3, image 0, rien a l'ecran |
| rayman-raving-rabbids-2 | YRRE | 16 | ok | plante : saut vers une adresse DS sans fonction : 20a7650, rien a l'ecran |
| rayman-rr | AR5E | 0 | ok | tourne, image 1140, affiche |
| resident-evil-deadly-silence | ABHE | 0 | ok | tourne, image 0, rien a l'ecran |
| saga-3-jikuu-no-hasha-shadow-or-light | BZ3J | 6 | ok | plante : instruction non traduite func_02013904 @2013914, rien a l'ecran |
| shrek-forever-after | VSKV | 2 | ok | plante : acces memoire invalide 42001e8 (ecriture) eip=10002e9a esp=eff50 arm=0, rien a l'ecran |
| simanimals | CM3P | 0 | ok | tourne, image 0, rien a l'ecran |
| simcity-creator | YC2P | 16 | ok | plante : acces memoire invalide 83e589b9 (ecriture) eip=10009d80 esp=20fff6e0 a, rien a l'ecran |
| simpsons-game | YSZE | 2 | ok | plante : saut vers une adresse DS sans fonction : 2002c28, rien a l'ecran |
| sims-2-the | ASJP | 19 | ok | tourne, image 0, rien a l'ecran |
| spider-man-2-eu | AS2P | 0 | ok | tourne, image 0, rien a l'ecran |
| spider-man-2-usa | AS2E | 0 | ok | tourne, image 0, rien a l'ecran |
| spider-man-shattered-dimensions | B3PP | 0 | ok | plante : instruction non traduite func_0210ece4 @210ecf4, rien a l'ecran |
| spider-man-web-of-shadows | CS5X | 0 | ok | plante : saut vers une adresse DS sans fonction : 200453c, rien a l'ecran |
| spore-creatures | YSPE | 7 | ok | plante : acces memoire invalide 83e589b9 (ecriture) eip=10009d80 esp=20fff830 a, rien a l'ecran |
| spyro-shadow-legacy | ASSE | 0 | ok | tourne, image 0, rien a l'ecran |
| star-wars-episode-iii-revenge-of-the-sit | ASTP | 30 | ok | tourne, image 0, rien a l'ecran |
| star-wars-lethal-alliance | AWUP | 30 | ok | tourne, image 0, rien a l'ecran |
| star-wars-the-clone-wars-republic-heroes | BW9E | 0 | ok | plante : instruction non traduite func_02056678 @2056688, rien a l'ecran |
| thrillville-off-the-rails | YTVE | 26 | ok | tourne, image 0, rien a l'ecran |
| tom-and-jerry-tales | AJTP | 0 | ok | tourne, image 1140, affiche |
| toy-story-3-eu | VT3V | 1 | ok | plante : acces memoire invalide 42000a8 (ecriture) eip=10002e9a esp=eff50 arm=0, rien a l'ecran |
| ultimate-spider-man | AUSF | 105 | ok | tourne, image 0, rien a l'ecran |
| viva-pinata-pocket-paradise | CP3E | 12 | ok | plante : instruction non traduite func_0208b104 @208b114, rien a l'ecran |
