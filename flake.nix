{
  description = "mcserv — gestionnaire de serveurs Minecraft (backend Rocket)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      mcservPackage =
        pkgs:
        pkgs.rustPlatform.buildRustPackage {
          pname = "mcserv";
          version = "0.1.0";

          src = pkgs.lib.cleanSourceWith {
            src = ./.;
            filter =
              path: _type:
              let
                base = builtins.baseNameOf (toString path);
              in
              !(builtins.elem base [
                "target"
                ".git"
              ]);
          };

          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [ pkgs.openssl ];

          # Les tests PTY ont besoin d'un vrai terminal ; on les lance hors
          # du sandbox Nix.
          doCheck = false;

          meta = {
            description = "Gestionnaire de serveurs Minecraft et catalogue de mods";
            homepage = "https://github.com/Xmoncoco/mc_server_ocherstrator";
            mainProgram = "mcserv";
            platforms = nixpkgs.lib.platforms.linux;
          };
        };
    in
    {
      packages = forAllSystems (
        pkgs:
        let
          mcserv = mcservPackage pkgs;
        in
        {
          inherit mcserv;
          default = mcserv;
        }
      );

      nixosModules.default = import ./nix/module.nix { inherit self; };


      devShells = forAllSystems (
        pkgs:
        let
          tauriLibs = with pkgs; [
            webkitgtk_4_1
            gtk3
            libsoup_3
            glib
            dbus
            librsvg
            openssl
          ];
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              rustc
              cargo
              rustfmt
              clippy
              pkg-config
              nodejs
              bun
              patchelf
              wrapGAppsHook3
            ];
            buildInputs = tauriLibs;
          };
        }
      );

      formatter = forAllSystems (pkgs: pkgs.nixfmt-rfc-style);
    };
}
