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
    in
    {
      packages = forAllSystems (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "wl-party";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
          src = self;
          cargoLock.lockFile = ./Cargo.lock;

          meta = {
            description = "Click-through confetti overlay for Wayland compositors";
            homepage = "https://github.com/kjvdven/wl-party";
            license = pkgs.lib.licenses.mit;
            mainProgram = "wl-party";
            platforms = pkgs.lib.platforms.linux;
          };
        };
      });
    };
}
