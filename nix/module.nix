{ self }:
{ config, lib, pkgs, ... }:

let
  cfg = config.services.kconfigwtf;
  package = cfg.package;
  listenHost =
    if lib.hasInfix ":" cfg.listenAddress
    then "[${cfg.listenAddress}]"
    else cfg.listenAddress;
  listen = "${listenHost}:${toString cfg.port}";
in
{
  options.services.kconfigwtf = {
    enable = lib.mkEnableOption "the kconfigwtf backend";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = lib.literalExpression "inputs.kconfigwtf.packages.\${pkgs.system}.default";
      description = "The kconfigwtf package to run.";
    };

    dataDir = lib.mkOption {
      type = lib.types.str;
      default = "${package}/share/kconfigwtf/data";
      defaultText = lib.literalExpression ''"\${config.services.kconfigwtf.package}/share/kconfigwtf/data"'';
      description = ''
        Directory containing package indexes and raw configs. Point this at the
        data directory in a server-side Git checkout to update data with git pull.
      '';
    };

    listenAddress = lib.mkOption {
      type = lib.types.str;
      default = "127.0.0.1";
      description = "Address on which the backend listens.";
    };

    port = lib.mkOption {
      type = lib.types.port;
      default = 3000;
      description = "Port on which the backend listens.";
    };

    title = lib.mkOption {
      type = lib.types.str;
      default = "kconfigwtf";
      description = "Title shown in the browser frontend.";
    };

    nginx.virtualHost = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "kconfigwtf.example.org";
      description = "Optional nginx virtual host to configure as a reverse proxy.";
    };
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      systemd.services.kconfigwtf = {
        description = "kconfigwtf backend";
        wantedBy = [ "multi-user.target" ];
        after = [ "network.target" ];

        serviceConfig = {
          ExecStart = lib.escapeShellArgs [
            (lib.getExe package)
            "serve"
            "--data-dir"
            cfg.dataDir
            "--listen"
            listen
            "--title"
            cfg.title
          ];
          DynamicUser = true;
          Restart = "on-failure";
          RestartSec = "5s";
          NoNewPrivileges = true;
          PrivateTmp = true;
          ProtectHome = true;
          ProtectSystem = "strict";
          ProtectKernelLogs = true;
          ProtectKernelModules = true;
          ProtectKernelTunables = true;
          RestrictAddressFamilies = [ "AF_UNIX" "AF_INET" "AF_INET6" ];
          RestrictRealtime = true;
          LockPersonality = true;
          MemoryDenyWriteExecute = true;
          CapabilityBoundingSet = "";
        };
      };
    }

    (lib.mkIf (cfg.nginx.virtualHost != null) {
      services.nginx.enable = true;
      services.nginx.virtualHosts.${cfg.nginx.virtualHost}.locations."/" = {
        proxyPass = "http://${listen}";
        recommendedProxySettings = true;
      };
    })
  ]);
}
