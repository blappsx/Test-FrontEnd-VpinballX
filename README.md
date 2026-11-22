# Test FrontEnd VpinballX

Frontend Rust pour Windows. Il lit chaque sous-dossier direct de `tables` comme une table VPX. Il choisit en priorité le fichier VPX portant le nom du dossier, ou l’unique fichier VPX du dossier. Avec plusieurs VPX sans correspondance exacte, le dossier est ignoré et signalé dans la console.

## Démarrage

Installer Rust stable 1.95 ou plus récent avec rustup et les outils de compilation C++ de Visual Studio pour Windows. FFmpeg et FFplay doivent être dans le PATH ou dans le même dossier que le chemin FFmpeg indiqué dans `config.toml`.

Ouvrir PowerShell dans le dossier du projet, vérifier les chemins de `config.toml`, puis exécuter :

```powershell
Copy-Item .\config.example.toml .\config.toml
# Adapter les chemins dans config.toml avant de lancer.
cargo run --release -- .\config.toml
```

Pour démarrer directement depuis l’exécutable :

```powershell
Copy-Item .\config.toml .\target\release\config.toml
.\target\release\pinready-frontend.exe
```

L’exécutable accepte aussi le chemin d’un autre fichier de configuration en premier argument. Les chemins relatifs sont résolus depuis le dossier de configuration, sauf un simple nom de commande FFmpeg, recherché dans le PATH. Conserver les apostrophes dans le TOML pour les chemins Windows.

## Utilisation

- **← / →** font défiler les wheels avec une animation de 375 ms par défaut. Le réglage `scroll_duration_ms` change cette durée. Une valeur inférieure accélère le défilement et `0` désactive l’animation. Deux voisines apparaissent de chaque côté, avec passage automatique de la dernière table à la première et inversement.
- Le carrousel et ses commandes disparaissent après `wheel_idle_seconds` sans touche (5 secondes par défaut, `0` pour les garder visibles). Une touche les réaffiche.
- `medias/audio.mp3` est joué en boucle pour la table sélectionnée. Le bouton en haut à droite coupe ou réactive le son pour toutes les tables jusqu’à la fermeture du frontend.
- **Entrée** lance VPX avec le fichier sélectionné. Le backglass du frontend se ferme pendant la partie et revient à la fermeture de VPX.
- **Échap** ferme le frontend.
- `medias/launcher.png` fournit le wheel. Le nom de la table s’affiche si l’image manque.
- Un fondu court relie les aperçus. La vidéo de fond tourne en boucle sans son et est rendue à la résolution du playfield. La console indique le chemin de la vidéo chargée et la résolution du rendu. Elle est centrée à la plus grande taille possible dans le playfield, avec des bandes noires si nécessaire pour garder toute l’image et ses proportions. Le réglage `background_video` indique le nom du fichier dans `medias`. Par défaut, il utilise `table.original.mp4`. Pour utiliser la version préparée, renseigner `background_video = 'table.mp4'`.
- `medias/bg.png` s’affiche en plein écran sur le deuxième moniteur, avec conservation des proportions. En l’absence du fichier l’écran reste noir.

Les indices d’écran démarrent à 0 et suivent l’ordre de détection du frontend. Si les écrans sont inversés, permuter les valeurs dans `config.toml`. Supprimer la ligne `backglass_monitor` pour n’afficher que le playfield.

Dans `arguments`, garder `{table}` comme argument séparé. Le frontend transmet le chemin complet à VPX, y compris s’il contient des espaces.
