-- Passer l'intro de Lego Star Wars II (version USA), en script : des que le jeu a ouvert son premier ecran, on ouvre les deux
-- ecrans du menu, comme a la fin de l'intro. Meme chose que le mod natif skip-intro, sans compilateur ni release : le depot suffit.
--
-- La table nds donne au script les memes acces qu'un mod natif (docs du SDK). Le script tourne dans un bac a sable : ni fichiers,
-- ni reseau, sauf permission demandee dans nds-mod.json et acceptee par le joueur.

local SCREENS = 0x02102900      -- gestionnaire des ecrans : non nul une fois le premier ecran ouvert
local OPEN_SCREEN = 0x0205ecc4  -- la fonction du jeu qui ouvre un ecran
local MENU_SCREEN, MENU_ARG, BACK_SCREEN = 0x11, 4, 0x0f

local done = false

-- dans une interruption, comme le mod natif : le jeu est entre deux ecrans
nds.on_irq(function(bits)
	if done or nds.read32(SCREENS) == 0 then
		return
	end
	done = true
	nds.call(OPEN_SCREEN, SCREENS, MENU_SCREEN, 0, MENU_ARG)
	nds.call(OPEN_SCREEN, SCREENS, BACK_SCREEN, 1, 0)
	nds.log("menu ouvert a l'image", nds.frame())
end)
