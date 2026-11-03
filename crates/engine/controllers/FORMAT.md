# Format d'une manette (`<id>.toml`)

Chaque fichier décrit une manette : comment la reconnaître quand elle est branchée, et comment la
dessiner dans le menu Options > Manettes. Le dessin est fait de rectangles à coins arrondis
(un cercle = un carré avec `radius = w / 2`). Pendant qu'on appuie sur une touche de la vraie
manette, la touche dessinée s'allume.

## Toile

- 480 x 300 unités, origine en haut à gauche, x vers la droite, y vers le bas.
- Laisser une marge d'au moins 10 unités autour de la manette.
- La manette vue de face ; les gâchettes et boutons de tranche (L1/R1, L2/R2, ZL/ZR...) sont dessinés
  au-dessus du corps, en haut de la toile (vus « de dessus » à plat), comme sur les schémas des
  notices.

## En-tête

```toml
id = "xbox-series"                      # = nom du fichier, minuscules et tirets
name = "Xbox One / Series Controller"   # affiché dans le menu déroulant (anglais)
name_fr = "Manette Xbox One / Series"   # facultatif : le nom en français
# Reconnaissance d'une manette branchée (au moins une des deux listes)
names = ["xbox one", "xbox series", "xbox wireless"]   # morceaux du nom donné par le pilote, en minuscules
vendor_ids = [0x045E]                   # identifiant USB du fabricant
product_ids = [0x02EA, 0x0B12]          # facultatif : identifiants USB du modèle
```

## Corps : `[[part]]` (dessinés dans l'ordre, avant les touches)

```toml
[[part]]
x = 70        # coin haut gauche
y = 70
w = 340       # taille
h = 170
radius = 80   # rayon des coins (0 = carré, w/2 = cercle si w = h)
color = "#2b2d31"
```

Plusieurs parts se superposent pour faire la forme (corps + poignées + zone centrale...).
Pas de texte dans les parts.

## Touches : `[[button]]`

```toml
[[button]]
input = "South"        # nom de la touche dans Bevy (liste plus bas)
label = "A"            # texte écrit sur la touche (court : 1 à 6 caractères), peut être ""
x = 330
y = 150
w = 28
h = 28
radius = 14
color = "#3a9a3a"      # couleur de la touche au repos
label_color = "#ffffff"
```

Touches Bevy (`input`) — mettre toutes celles que la manette possède :

| input | Rôle (disposition « Xbox ») |
| --- | --- |
| `South` `East` `West` `North` | les 4 boutons de face : bas, droite, gauche, haut (Xbox A B X Y, PlayStation Croix Rond Carré Triangle, Nintendo B A Y X) |
| `LeftTrigger` `RightTrigger` | boutons de tranche L1 / R1, LB / RB, L / R |
| `LeftTrigger2` `RightTrigger2` | gâchettes L2 / R2, LT / RT, ZL / ZR |
| `Select` `Start` | View / Menu, Share / Options, Create / Options, - / + |
| `Mode` | bouton central (Xbox, PS, Home) |
| `LeftThumb` `RightThumb` | appui sur les sticks (L3 / R3) : ne pas dessiner, le stick s'allume |
| `DPadUp` `DPadDown` `DPadLeft` `DPadRight` | la croix : 4 touches séparées |
| `C` `Z` | boutons en plus (rares : Capture, boutons arrière) |

## Sticks : `[[stick]]`

```toml
[[stick]]
side = "left"        # left | right
x = 110              # coin haut gauche du socle
y = 110
size = 56            # diamètre du socle
color = "#1c1d20"    # socle
cap_color = "#3b3e44" # chapeau (bouge avec le stick, s'allume quand on l'enfonce)
```

## Règles

- Les touches ne se chevauchent pas ; tailles réalistes (boutons de face ~26-32, croix ~22 par branche).
- Couleurs proches de la vraie manette (corps, couleurs ou symboles des boutons).
- Les labels : lettres ou mots courts. Pas de symboles Unicode (la police n'a que l'ASCII et les
  accents) : PlayStation = `X`, `O`, `[]`, `/\` ou les mots `Croix`, `Rond`... au choix, mais court.
- Un seul fichier par manette, et rien d'autre à modifier.
- Vérifier que le fichier se lit : `python -c "import tomllib;tomllib.load(open('<id>.toml','rb'))"`.
