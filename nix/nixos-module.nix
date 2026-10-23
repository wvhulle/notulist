notulistFlake:
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.programs.notulist;
  inherit (import ./whisper-models.nix)
    whisperModelNames
    defaultWhisperModelForAcceleration
    fetchWhisperModel
    ;
  tomlFormat = pkgs.formats.toml { };
  hostSystem = pkgs.stdenv.hostPlatform.system;

  modelSettings = lib.optionalAttrs (cfg.whisperModel != null) {
    daemon.whisper.model = "${fetchWhisperModel pkgs.fetchurl cfg.whisperModel}";
  };

  notulistConfigFile = tomlFormat.generate "notulist-config.toml" (
    lib.recursiveUpdate modelSettings cfg.settings
  );

  configuredVideoDrivers = config.services.xserver.videoDrivers;

  detectedAcceleration =
    if lib.elem "nvidia" configuredVideoDrivers then
      "cuda"
    else if lib.elem "amdgpu" configuredVideoDrivers then
      "vulkan"
    else
      "cpu";

  graphicalSessionUnit = {
    partOf = [ "graphical-session.target" ];
    wantedBy = [ "graphical-session.target" ];
    restartTriggers = [ notulistConfigFile ];
  };
in
{
  options.programs.notulist = {
    enable = lib.mkEnableOption "notulist streaming whisper dictation for every graphical session";

    acceleration = lib.mkOption {
      type = lib.types.enum [
        "cpu"
        "cuda"
        "vulkan"
      ];
      default = detectedAcceleration;
      defaultText = lib.literalMD ''
        `cuda` when `services.xserver.videoDrivers` contains `nvidia`, `vulkan` when it contains
        `amdgpu`, otherwise `cpu`
      '';
      description = ''
        Which prebuilt whisper.cpp from nixpkgs notulist links against. Set `vulkan` explicitly
        for Intel GPUs that handle it well.
      '';
    };

    package = lib.mkOption {
      type = lib.types.package;
      default = notulistFlake.packages.${hostSystem}.${cfg.acceleration};
      defaultText = lib.literalExpression "notulist.packages.\${system}.\${config.programs.notulist.acceleration}";
      description = "The notulist package providing the `notulist` and `notulist-ime` binaries.";
    };

    whisperModel = lib.mkOption {
      type = lib.types.nullOr (lib.types.enum whisperModelNames);
      default = defaultWhisperModelForAcceleration.${cfg.acceleration};
      defaultText = lib.literalMD "`large-v3-turbo` for `cuda`, `small` for `vulkan`, `base` for `cpu`";
      example = "medium";
      description = ''
        Whisper model to download into the Nix store and pass as `[daemon.whisper] model`.
        Set to null to point `settings.daemon.whisper.model` at your own file instead.
      '';
    };

    settings = lib.mkOption {
      inherit (tomlFormat) type;
      default = { };
      example = lib.literalExpression ''
        {
          daemon.whisper.languages = [ "en" "nl" ];
          daemon.streaming.revise_recent_words = true;
          ime.popup.level_meter.bar_count = 8;
          ime.popup.phase_styles.hearing.colour = "#88c0d0";
        }
      '';
      description = ''
        Contents of the notulist config file shared by the daemon (`daemon` table) and the
        input method (`ime` table); see the notulist README for every key.
      '';
    };

    useCudaBinaryCache = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Trust the NixOS CUDA binary cache when `acceleration` is `cuda`, so the CUDA build of
        whisper.cpp is downloaded prebuilt instead of compiled.
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    nix.settings = lib.mkIf (cfg.useCudaBinaryCache && cfg.acceleration == "cuda") {
      substituters = [ "https://cache.nixos-cuda.org" ];
      trusted-public-keys = [ "cache.nixos-cuda.org:74DUi4Ye579gUqzH4ziL9IyiJBlDpMRn9MBN8oNan9M=" ];
    };

    environment.systemPackages = [ cfg.package ];

    environment.etc."xdg/notulist/config.toml".source = notulistConfigFile;

    hardware.uinput.enable = true;

    services.udev.extraRules = ''
      KERNEL=="uinput", SUBSYSTEM=="misc", TAG+="uaccess"
    '';

    systemd.user.services.notulist = graphicalSessionUnit // {
      description = "notulist dictation daemon";
      after = [
        "graphical-session.target"
        "pipewire.service"
      ];
      serviceConfig = {
        Type = "dbus";
        BusName = "io.github.wvhulle.Notulist";
        ExecStart = "${lib.getExe' cfg.package "notulist"} --config ${notulistConfigFile} daemon";
        Restart = "on-failure";
        RestartSec = 2;
      };
    };

    systemd.user.services.notulist-ime = graphicalSessionUnit // {
      description = "notulist input method and dictation popup";
      after = [
        "graphical-session.target"
        "notulist.service"
      ];
      wants = [ "notulist.service" ];
      path = [ pkgs.dotool ];
      serviceConfig = {
        ExecStart = "${lib.getExe' cfg.package "notulist-ime"} --config ${notulistConfigFile}";
        Restart = "always";
        RestartSec = 2;
      };
    };
  };
}
