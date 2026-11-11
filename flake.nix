{
  description = "Small Hyprland workspace OSD";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = nixpkgs.legacyPackages.${system};
    in
    {
      packages.${system}.default = pkgs.rustPlatform.buildRustPackage {
        pname = "workspace-indicator";
        version = "0.1.0";
        src = self;
        cargoLock.lockFile = ./Cargo.lock;
        strictDeps = true;
        meta.mainProgram = "workspace-indicator";
      };

      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          cargo
          rustc
          rustfmt
          clippy
        ];
      };

      formatter.${system} = pkgs.nixfmt;
    };
}
