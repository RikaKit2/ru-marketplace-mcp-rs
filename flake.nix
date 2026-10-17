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
        packages = {
          default = pkgs.stdenv.mkDerivation {
            pname = "ru-marketplace-mcp";
            version = "0.2.0";
            src = ./dist;

            dontBuild = true;
            dontConfigure = true;

            installPhase = ''
              mkdir -p $out/bin
              cp ru-marketplace-mcp $out/bin/
              chmod +x $out/bin/ru-marketplace-mcp
            '';

            meta = {
              description = "Russian Marketplaces Price Comparison & MCP Server";
              mainProgram = "ru-marketplace-mcp";
            };
          };

          from-source = pkgs.rustPlatform.buildRustPackage {
            pname = "ru-marketplace-mcp";
            version = "0.2.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;

            meta = {
              description = "Russian Marketplaces Price Comparison & MCP Server";
              mainProgram = "ru-marketplace-mcp";
            };
          };
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
