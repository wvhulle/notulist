{
  description = "notulist: streaming whisper dictation daemon and Wayland input method";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    crane.url = "github:ipetkov/crane";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      crane,
      rust-overlay,
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
      ];

      nixpkgsFor =
        system: extraConfig:
        import nixpkgs {
          inherit system;
          config = {
            allowUnfree = true;
          }
          // extraConfig;
          overlays = [ (import rust-overlay) ];
        };

      forEachSystem = nixpkgs.lib.genAttrs supportedSystems;

      buildNotulist =
        system:
        let
          pkgs = nixpkgsFor system { };
          pkgsWithCuda = nixpkgsFor system { cudaSupport = true; };
          inherit (pkgs) lib;

          craneLib = (crane.mkLib pkgs).overrideToolchain (
            pkgs.rust-bin.stable.latest.default.override {
              extensions = [
                "rust-src"
                "rustfmt"
                "clippy"
                "rust-analyzer"
              ];
            }
          );

          whisperCppByAcceleration = {
            cpu = pkgs.whisper-cpp;
            cuda = pkgsWithCuda.whisper-cpp;
            vulkan = pkgs.whisper-cpp.override { vulkanSupport = true; };
          };

          imeRuntimeDependencies = [ pkgs.dotool ];

          buildArgsFor = whisperCpp: {
            pname = "notulist";
            inherit ((builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package) version;
            src = craneLib.cleanCargoSource ./.;
            strictDeps = true;
            cargoExtraArgs = "--locked";
            nativeBuildInputs = [
              pkgs.pkg-config
              pkgs.rustPlatform.bindgenHook
            ];
            buildInputs = [
              whisperCpp
              pkgs.alsa-lib
              pkgs.wayland
              pkgs.fontconfig
            ];
          };

          buildWorkspace =
            whisperCpp:
            let
              buildArgs = buildArgsFor whisperCpp;
            in
            craneLib.buildPackage (
              buildArgs
              // {
                cargoArtifacts = craneLib.buildDepsOnly buildArgs;
                doCheck = false;
                passthru.whisperCpp = whisperCpp;
                meta = {
                  description = "Streaming whisper dictation daemon and Wayland input method";
                  license = lib.licenses.mit;
                  mainProgram = "notulist";
                  platforms = supportedSystems;
                };
              }
            );

          whisperModels = import ./nix/whisper-models.nix;

          devShellFor =
            acceleration: whisperCpp:
            craneLib.devShell {
              inputsFrom = [ (buildWorkspace whisperCpp) ];
              NOTULIST_WHISPER_MODEL = "${whisperModels.fetchWhisperModel pkgs.fetchurl
                whisperModels.defaultWhisperModelForAcceleration.${acceleration}
              }";
              packages = imeRuntimeDependencies ++ [
                pkgs.cargo-nextest
                pkgs.imv
                pkgs.wayland-utils
                pkgs.wev
              ];
            };
        in
        {
          packages = lib.mapAttrs (_: buildWorkspace) whisperCppByAcceleration;
          devShells = lib.mapAttrs devShellFor whisperCppByAcceleration;
        };
    in
    {
      packages = forEachSystem (
        system:
        let
          notulist = buildNotulist system;
        in
        notulist.packages // { default = notulist.packages.cpu; }
      );

      devShells = forEachSystem (
        system:
        let
          notulist = buildNotulist system;
        in
        notulist.devShells // { default = notulist.devShells.vulkan; }
      );

      nixosModules.default = import ./nix/nixos-module.nix self;
    };
}
