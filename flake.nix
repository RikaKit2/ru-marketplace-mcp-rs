{
  description = "ru-marketplace-mcp-rs: Russian Marketplaces Price Comparison & MCP Server";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
      in
      {
        packages = rec {
          default = pkgs.rustPlatform.buildRustPackage {
            pname = "ru-marketplace-mcp";
            version = "0.2.1";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;

            preBuild = ''
              rm -rf .cargo
            '';

            meta = {
              description = "Russian Marketplaces Price Comparison & MCP Server";
              mainProgram = "ru-marketplace-mcp";
            };
          };

          from-source = default;
        };

        apps.default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/ru-marketplace-mcp";
        };

        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            prek
          ];
        };
      }
    );
}
