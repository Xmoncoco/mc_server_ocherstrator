{ self }:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.services.mcserv;
  dataDir = "/var/lib/mcserv";
in
{
  options.services.mcserv = {
    enable = lib.mkEnableOption "mcserv, gestionnaire de serveurs Minecraft";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.mcserv;
      description = "Paquet mcserv à exécuter.";
    };

    address = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1";
      description = ''
        Adresse d'écoute de l'API Rocket. Utilise "0.0.0.0" pour accepter
        les connexions distantes du client Tauri.
      '';
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 8000;
      description = "Port de l'API mcserv.";
    };

    openFirewall = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Ouvrir le port de l'API dans le pare-feu.";
    };

    javaPackage = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = null;
      description = ''
        Java à ajouter au PATH du service. Laisse null si Java est déjà
        installé sur le système ou si chaque serveur utilise un chemin
        absolu configuré dans l'interface.
      '';
    };

    environmentFile = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      example = "/etc/mcserv.env";
      description = ''
        Fichier d'environnement systemd, utile pour
        CURSEFORGE_API_KEY=... ou d'autres secrets.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    users.groups.mcserv = { };

    users.users.mcserv = {
      isSystemUser = true;
      group = "mcserv";
      home = dataDir;
    };

    systemd.services.mcserv = {
      description = "mcserv — gestionnaire de serveurs Minecraft";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];

      environment = {
        ROCKET_ADDRESS = cfg.address;
        ROCKET_PORT = toString cfg.port;
        MCSSERV_DATA_DIR = dataDir;
        HOME = dataDir;
        # Requis pour les téléchargements HTTPS (Modrinth/CurseForge) depuis
        # un service systemd NixOS.
        SSL_CERT_FILE = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt";
      };

      path = lib.optional (cfg.javaPackage != null) cfg.javaPackage;

      serviceConfig = {
        User = "mcserv";
        Group = "mcserv";
        ExecStart = "${cfg.package}/bin/mcserv";
        Restart = "on-failure";
        RestartSec = 5;

        StateDirectory = "mcserv";
        WorkingDirectory = dataDir;
        EnvironmentFile = lib.optional (cfg.environmentFile != null) cfg.environmentFile;

        # Laisse aux serveurs le temps de s'arrêter proprement (SIGTERM).
        TimeoutStopSec = 40;
        KillMode = "control-group";

        # Durcissement raisonnable, sans bloquer les sous-processus Java.
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = "read-only";
        ReadWritePaths = [ dataDir ];
        LimitNOFILE = 65536;
      };
    };

    networking.firewall.allowedTCPPorts = lib.mkIf cfg.openFirewall [ cfg.port ];
  };
}
