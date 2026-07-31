{
  description = "A safe, Git-aware NixOS lifecycle CLI";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      packageFor =
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          cargoToml = builtins.fromTOML (builtins.readFile ./Cargo.toml);
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = "nr";
          version = cargoToml.package.version;
          src = nixpkgs.lib.cleanSource ./.;

          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [
            pkgs.installShellFiles
            pkgs.makeWrapper
          ];
          nativeCheckInputs = [ pkgs.git ];

          postInstall = ''
            installManPage man/nr.1
          '';

          postFixup = ''
            wrapProgram $out/bin/nr \
              --prefix PATH : ${
                pkgs.lib.makeBinPath [
                  pkgs.gh
                  pkgs.git
                  pkgs.nix
                  pkgs.nix-output-monitor
                  pkgs.nixos-rebuild
                  pkgs.nixfmt
                  pkgs.statix
                ]
              }
          '';
        };
    in
    {
      packages = forAllSystems (system: {
        default = packageFor system;
        nr = packageFor system;
      });

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/nr";
          meta.description = "Build, switch, update, check, and publish a NixOS flake";
        };
      });

      checks = forAllSystems (system: {
        package = self.packages.${system}.default;
      });

      devShells = forAllSystems (
        system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in
        {
          default = pkgs.mkShellNoCC {
            packages = with pkgs; [
              cargo
              clippy
              gh
              git
              nix
              nix-output-monitor
              nixos-rebuild
              nixfmt
              rustc
              rustfmt
              statix
            ];
          };
        }
      );

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt);
    };
}
