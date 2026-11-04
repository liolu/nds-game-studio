# Jeux natifs : ce que donne chaque jeu

Relevé automatique de `native/tools/build_all.py`, sans aucune touche pressée : un essai court ne dit pas qu'un jeu est jouable.
Les modules restent sur le PC (`target/release/native`), jamais dans git.
La colonne « Essai » vient d'un relevé sous Windows ; la colonne « macOS » est tenue à la main (`MACOS` dans `build_all.py`).

| Jeu | Code | Overlays | Construction | Essai (Windows) | macOS |
|---|---|---|---|---|---|
| battle-of-giants-dinosaurs | CD5E | 12 | ok | plante : instruction non traduite func_0203f8b4 @203f8c4, rien a l'ecran | pas essaye |
| blue-dragon-awakened-shadow | YBUD | 16 | ok | plante : instruction non traduite __sinit_ov000_021e751c @21e751c, rien a l'ecran | pas essaye |
| blue-dragon-plus | YYBP | 123 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| children-of-mana | ASEE | 16 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| club-house-games | ATDE | 363 | ok | plante : saut vers une adresse DS sans fonction : 205faad, rien a l'ecran | pas essaye |
| code-lyoko | AL5E | 16 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| cooking-mama | ACCE | 0 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| crafting-mama | BQ8E | 164 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| dawn-of-heroes | CIEE | 12 | ok | plante : instruction non traduite func_02094588 @2094598, rien a l'ecran | pas essaye |
| dragon-ball-origins | CDZP | 0 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| dragon-ball-z-attack-of-the-saiyans | BRPP | 0 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| eyeshield-21 | AE4J | 10 | ok | plante : acces memoire invalide 1000e56c (ecriture) eip=1010ed98 esp=20fffd30 a, rien a l'ecran | pas essaye |
| fairy-tail-kardia | TFTJ | 241 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| fairy-tail-madoushi | BH6J | 68 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| farm-frenzy | BFZE | 0 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| harry-potter-and-the-deathly-hallows-par | B7HE | 4 | ok | plante : acces memoire invalide 0 (ecriture) eip=0 esp=20fff864 arm=0, rien a l'ecran | pas essaye |
| harvest-moon-ds | ABCE | 0 | ok | essaye jusqu'en jeu par le launcher : intro, nom, ferme, sauvegarde (Diary) ; noms SDK dans patches.json | pas essaye |
| heroes-of-mana | AHEP | 124 | ok | essaye jusqu'en jeu par le launcher : chapitre 1, bataille, lasso ; overlays multi, stack_args 32, video d'intro sautee | pas essaye |
| how-to-train-your-dragon | BHDP | 0 | ok | plante : instruction non traduite func_020ce780 @20ce790, rien a l'ecran | pas essaye |
| iron-man-2 | B2IE | 2 | ok | plante : instruction non traduite func_0216a3bc @216a3cc, rien a l'ecran | pas essaye |
| lego-star-wars-ii-the-original-trilogy | AL7E | 0 | ok | tourne, image 1080, affiche | demarre sans fenetre jusqu'a l'ecran titre (arm64 et x86_64) ; ecran du launcher : pas verifie |
| livly-garden | BLIJ | 27 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| luminous-arc-2 | YL2P | 4 | ok | plante : acces memoire invalide 83e589b9 (ecriture) eip=10009d80 esp=20fffa88 a, rien a l'ecran | pas essaye |
| madagascar | AMGP | 0 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| mysims | AQIE | 17 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| myst | YMYE | 0 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| nintendogs-chihuahua-friends | AD2J | 28 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| nintendogs-dachshund | ADGE | 30 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| nintendogs-miniature-dachshund-friends | ADGJ | 28 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| nintendogs-shiba-friends | AD3J | 28 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| penguins-of-madagascar-the | VPEV | 0 | ok | plante : acces memoire invalide 42014f0 (ecriture) eip=10002e9a esp=eff50 arm=0, rien a l'ecran | pas essaye |
| plants-vs-zombies | BWBE | 3 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| plants-vs-zombies-eu | BWBY | 3 | ok | plante : acces memoire invalide 13822010 (ecriture) eip=1019b282 esp=20fff758 a, rien a l'ecran | pas essaye |
| plants-vs-zombies-eu-2 | BWBX | 3 | ok | plante : acces memoire invalide 13822010 (ecriture) eip=1019b302 esp=20fff758 a, rien a l'ecran | pas essaye |
| pokemon-dash | APDE | 2 | ok | essaye jusqu'en course, 60 images/s, stylet ; une course entiere et la sauvegarde pas encore essayees | pas essaye |
| pokemon-ranger | ARGE | 16 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| powerpuff-girls-z | YPPJ | 0 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| professor-layton-and-the-spectre-s-call | BLFP | 47 | ok | fin 3, image 0, rien a l'ecran | pas essaye |
| rayman-raving-rabbids-2 | YRRE | 16 | ok | plante : saut vers une adresse DS sans fonction : 20a7650, rien a l'ecran | pas essaye |
| rayman-rr | AR5E | 0 | ok | tourne, image 1140, affiche | pas essaye |
| resident-evil-deadly-silence | ABHE | 0 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| saga-3-jikuu-no-hasha-shadow-or-light | BZ3J | 6 | ok | plante : instruction non traduite func_02013904 @2013914, rien a l'ecran | pas essaye |
| shrek-forever-after | VSKV | 2 | ok | plante : acces memoire invalide 42001e8 (ecriture) eip=10002e9a esp=eff50 arm=0, rien a l'ecran | pas essaye |
| simanimals | CM3P | 0 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| simcity-creator | YC2P | 16 | ok | plante : acces memoire invalide 83e589b9 (ecriture) eip=10009d80 esp=20fff6e0 a, rien a l'ecran | pas essaye |
| simpsons-game | YSZE | 2 | ok | plante : saut vers une adresse DS sans fonction : 2002c28, rien a l'ecran | pas essaye |
| sims-2-the | ASJP | 19 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| spider-man-2-eu | AS2P | 0 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| spider-man-2-usa | AS2E | 0 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| spider-man-shattered-dimensions | B3PP | 0 | ok | plante : instruction non traduite func_0210ece4 @210ecf4, rien a l'ecran | pas essaye |
| spider-man-web-of-shadows | CS5X | 0 | ok | plante : saut vers une adresse DS sans fonction : 200453c, rien a l'ecran | pas essaye |
| spore-creatures | YSPE | 7 | ok | plante : acces memoire invalide 83e589b9 (ecriture) eip=10009d80 esp=20fff830 a, rien a l'ecran | pas essaye |
| spyro-shadow-legacy | ASSE | 0 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| star-wars-episode-iii-revenge-of-the-sit | ASTP | 30 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| star-wars-lethal-alliance | AWUP | 30 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| star-wars-the-clone-wars-republic-heroes | BW9E | 0 | ok | plante : instruction non traduite func_02056678 @2056688, rien a l'ecran | pas essaye |
| thrillville-off-the-rails | YTVE | 26 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| tom-and-jerry-tales | AJTP | 0 | ok | tourne, image 1140, affiche | pas essaye |
| toy-story-3-eu | VT3V | 1 | ok | plante : acces memoire invalide 42000a8 (ecriture) eip=10002e9a esp=eff50 arm=0, rien a l'ecran | pas essaye |
| ultimate-spider-man | AUSF | 105 | ok | tourne, image 0, rien a l'ecran | pas essaye |
| viva-pinata-pocket-paradise | CP3E | 12 | ok | plante : instruction non traduite func_0208b104 @208b114, rien a l'ecran | pas essaye |
| wheel-of-fortune | BW7E | 0 | ok | plante : instruction non traduite func_020ade24 @20ade34, rien a l'ecran | pas essaye |
