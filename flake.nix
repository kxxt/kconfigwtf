{
  description = "kconfigwtf backend, data, and NixOS service";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
    in
    {
      packages = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        rec {
          kconfigwtf = pkgs.rustPlatform.buildRustPackage {
            pname = "kconfigwtf";
            version = "0.1.0";

            src = pkgs.lib.cleanSourceWith {
              src = ./.;
              filter = path: type:
                let
                  name = builtins.baseNameOf path;
                in
                !(builtins.elem name [ ".git" ".ci" "data" "public" "target" ]);
            };

            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = [ pkgs.pkg-config ];
            buildInputs = [ pkgs.bzip2 pkgs.xz ];
            doCheck = false;

            postInstall = ''
              mkdir -p "$out/share/kconfigwtf"
              cp -r ${./data} "$out/share/kconfigwtf/data"
            '';

            meta = {
              description = "Linux kernel config explorer and distribution config indexer";
              homepage = "https://github.com/kxxt/kconfigwtf";
              license = with pkgs.lib.licenses; [ mit asl20 ];
              mainProgram = "kconfigwtf";
            };
          };

          default = kconfigwtf;
        }
      );

      nixosModules = {
        kconfigwtf = import ./nix/module.nix { inherit self; };
        default = self.nixosModules.kconfigwtf;
      };

      formatter = forAllSystems (system: nixpkgs.legacyPackages.${system}.nixfmt-tree);
    };
}
