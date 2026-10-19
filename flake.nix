{
  description = "Click-through confetti overlay for Wayland compositors";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs [ "x86_64-linux" "aarch64-linux" ] (
          system: f nixpkgs.legacyPackages.${system}
        );
      version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
      meta = pkgs: {
        description = "Click-through confetti overlay for Wayland compositors";
        homepage = "https://github.com/kjvdven/wl-party";
        license = pkgs.lib.licenses.mit;
        mainProgram = "wl-party";
        platforms = pkgs.lib.platforms.linux;
      };
      releaseBinaries = {
        x86_64-linux = {
          asset = "wl-party-x86_64-linux";
          hash = "sha256-QMIBUTLo3jETfVVfqLLXX2j9MONL06PAhSDtaJckmHo=";
        };
        aarch64-linux = {
          asset = "wl-party-aarch64-linux";
          hash = "sha256-TGiscZsY83KZppEqiYFFCQmOKPmhqwYos1h7kjYBeMo=";
        };
      };
    in
    {
      packages = forAllSystems (
        pkgs:
        let
          inherit (pkgs.stdenv.hostPlatform) system;
          release = releaseBinaries.${system};
        in
        {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "wl-party";
            inherit version;
            src = self;
            cargoLock.lockFile = ./Cargo.lock;
            meta = meta pkgs;
          };

          bin = pkgs.stdenv.mkDerivation {
            pname = "wl-party-bin";
            inherit version;
            src = pkgs.fetchurl {
              url = "https://github.com/kjvdven/wl-party/releases/download/v${version}/${release.asset}";
              inherit (release) hash;
            };
            dontUnpack = true;
            installPhase = "install -Dm755 $src $out/bin/wl-party";
            meta = meta pkgs;
          };
        }
      );
    };
}
