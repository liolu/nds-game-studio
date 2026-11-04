# Trouver une adresse

Une accroche demande l'adresse d'une fonction du jeu (son début dans la mémoire du DS), et changer une valeur demande l'adresse de cette valeur. Les adresses sont celles d'une version du jeu : listez le SHA-1 de sa rom dans `"games"`.

## Fonctions

- **Le projet de décompilation du jeu**, s'il y en a un : `config/<version>/arm9/symbols.txt` donne chaque fonction, avec son nom quand quelqu'un l'a trouvé (`OS_GetTick kind:function(arm,size=0xa0) addr:0x020ce6c8`), et `src/` son C. Les overlays ont leur propre `symbols.txt`.
- **La recette du jeu** dans le runtime (`native/games/<code>.json.gz`) : chaque fonction traduite, `[adresse, taille, thumb, nom]`. Toutes ces adresses s'accrochent.
- **La fenêtre Code du launcher (F5)** montre les sources du jeu quand la décompilation est installée.
- **Un désassembleur** (Ghidra avec dsd-ghidra, IDA, Binary Ninja) sur la rom, pour lire le code ARM et suivre qui appelle quoi.
- `api->hook` rend -1 si l'adresse n'est pas le début d'une fonction traduite : le journal le dit.

Les bonnes fonctions à accrocher sont celles que le jeu appelle une fois par image ou une fois par objet : la mise à jour de la caméra, celle d'un personnage, la lecture des touches. L'accroche reçoit les mêmes `r0`-`r3` que le code du jeu, souvent `r0` = l'objet.

## Valeurs

- **Regarder la mémoire en jouant** : `--peek "image:adresse:taille:fichier"` copie la mémoire dans un fichier à une image donnée (voir [essayer.md](essayer.md)). Comparez deux copies : ce qui a changé quand le personnage a perdu une vie, c'est le compteur.
- **Qui écrit une valeur** : un module instrumenté (`native_build.py --instrument`) avec `--watch adresse` écrit dans le journal l'instruction ARM qui écrit là.
- **Structures** : un pointeur trouvé dans le code d'une fonction (`ldr r0, =0x02109cd4`) est souvent un objet global ; ses champs sont les décalages que lisent les fonctions qui s'en servent.
- `api->runtime("nom")` donne toute fonction ou variable du runtime lui-même (`nds_frame`, le compteur d'images). Sans promesse : un nom peut disparaître dans un runtime suivant ; vérifiez que ce n'est pas 0.

## Instructions

`api->hook_insn` accroche une instruction à l'intérieur d'une fonction et donne au rappel les registres ARM (`nds_regs`), modifiables. Déclarez l'adresse dans `"instruction_hooks"` pour ce jeu : le launcher regénère le module avec elle. Préférez les accroches de fonctions quand c'est possible : elles ne demandent rien.
