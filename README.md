# mcserv

Backend Rust (Rocket) de gestion de serveurs Minecraft : installation des
serveurs et mod loaders (Vanilla, Paper, Fabric, Quilt, Forge, NeoForge),
catalogue Modrinth/CurseForge, mods et modpacks, console PTY et WebSocket.

## Compilation et lancement

```bash
cargo build --release
./target/release/mcserv
```

Variables d'environnement :

- `ROCKET_ADDRESS` (défaut `127.0.0.1`), `ROCKET_PORT` (défaut `8000`) ;
- `MCSSERV_DATA_DIR` : dossier de données (sinon `$XDG_DATA_HOME/mcserv` ou
  `~/.local/share/mcserv`) ;
- `CURSEFORGE_API_KEY` : optionnelle, peut aussi être configurée via l'API.

## Flake Nix / NixOS

Le flake est à la racine de ce dépôt :
[`github:Xmoncoco/mc_server_ocherstrator`](https://github.com/Xmoncoco/mc_server_ocherstrator).

```bash
nix build github:Xmoncoco/mc_server_ocherstrator#mcserv   # ./result/bin/mcserv
nix run github:Xmoncoco/mc_server_ocherstrator
nix develop github:Xmoncoco/mc_server_ocherstrator        # Rust, Bun/Node, libs Tauri
```

En local, `nix build .#mcserv` fonctionne si les fichiers sont suivis par git
(`git add -A`).

### Module NixOS

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    mcserv.url = "github:Xmoncoco/mc_server_ocherstrator";
  };

  outputs = { self, nixpkgs, mcserv, ... }: {
    nixosConfigurations.ma-machine = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        ./configuration.nix
        mcserv.nixosModules.default

        ({ pkgs, ... }: {
          services.mcserv = {
            enable = true;
            address = "0.0.0.0";       # accès distant du client
            port = 8000;
            openFirewall = true;
            javaPackage = pkgs.jdk21;  # ou null si Java est déjà dans le PATH
            environmentFile = "/etc/mcserv.env"; # CURSEFORGE_API_KEY=...
          };
        })
      ];
    };
  };
}
```

Puis `sudo nixos-rebuild switch --flake .#ma-machine`.

Options disponibles : `enable`, `package`, `address`, `port`, `openFirewall`,
`javaPackage`, `environmentFile`.

Le service tourne sous l'utilisateur `mcserv`, stocke ses données dans
`/var/lib/mcserv` et applique un durcissement systemd
(`ProtectSystem=strict`, `PrivateTmp`, `NoNewPrivileges`, etc.).

Points d'attention :

- `userdb.toml` est à créer dans `/var/lib/mcserv` avec les clés publiques
  Ed25519 des clients ;
- `javaPackage` doit être défini (par exemple `pkgs.jdk21`) ou chaque serveur
  doit utiliser un chemin Java absolu : NixOS n'a pas de `/usr/bin/java` ;
- `openFirewall` n'ouvre que le port de l'API ; les ports de jeu se règlent
  dans `networking.firewall` ;
- les serveurs Java (Vanilla, Paper, Fabric, Quilt, Forge, NeoForge)
  fonctionnent tels quels ; un exécutable natif précompilé nécessite
  `nix-ld` ou `patchelf`.
