{
  description = "A safe, Git-aware NixOS rebuild helper";

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
          version = builtins.head (
            builtins.match ''__version__ = "([^"]+)"[[:space:]]*'' (builtins.readFile ./src/nr/version.py)
          );
        in
        pkgs.python3Packages.buildPythonApplication {
          pname = "nr";
          inherit version;
          pyproject = true;
          src = nixpkgs.lib.cleanSource ./.;

          build-system = [ pkgs.python3Packages.setuptools ];
          nativeBuildInputs = [ pkgs.makeWrapper ];
          nativeCheckInputs = [ pkgs.git ];

          pythonImportsCheck = [ "nr" ];
          checkPhase = ''
            runHook preCheck
            PYTHONPATH=src python -m unittest discover -s tests -p 'test_*.py' -v
            runHook postCheck
          '';

          postFixup = ''
            wrapProgram $out/bin/nr \
              --prefix PATH : ${
                pkgs.lib.makeBinPath [
                  pkgs.gh
                  pkgs.git
                  pkgs.nh
                  pkgs.nix
                  pkgs.nixfmt
                  pkgs.ruff
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
              gh
              git
              nh
              nix
              nixfmt
              python3
              ruff
              statix
            ];
          };
        }
      );

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt);
    };
}
