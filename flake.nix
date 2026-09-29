{
  description = "Pinned jev-dsl source and Node environment for web verification.";

  inputs.jev-dsl = {
    url = "github:inanna-malick/jev-dsl/f16f1363b4d389d6e34f9d695fbd254ca0735f2e";
    flake = false;
  };

  # Match Tidepool's pinned nixpkgs revision so web verification does not
  # depend on whichever Node installation happens to be on the host.
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/bfc1b8a4574108ceef22f02bafcf6611380c100d";
  inputs.rust-overlay = {
    url = "github:oxalica/rust-overlay/860d7c835ab91bfc8972b67092f5f2db8e9390a0";
    inputs.nixpkgs.follows = "nixpkgs";
  };

  # `[haskell.flake_sources]` in `.exomonad/config.toml` names the directories
  # inside jev-dsl that Exomonad captures as ordinary source roots.
  outputs = { nixpkgs, rust-overlay, ... }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forSystems = f: nixpkgs.lib.genAttrs systems (system: f (import nixpkgs {
        inherit system;
        overlays = [ (import rust-overlay) ];
      }));
    in {
      devShells = forSystems (pkgs: {
        web = pkgs.mkShell { packages = [ pkgs.nodejs_24 ]; };
      });

      packages = forSystems (pkgs: let
        buckReindeer = pkgs.rustPlatform.buildRustPackage {
          pname = "reindeer";
          version = "2026.09.14.00";
          src = pkgs.fetchFromGitHub {
            owner = "facebookincubator";
            repo = "reindeer";
            rev = "v2026.09.14.00";
            hash = "sha256-hVW+raZLZiZqOS0Qa+z9S/PWEyNpi2O05SVrmJOxG4c=";
          };
          cargoHash = "sha256-k4G7cZqtYLt0m0noIKhPl7B4rte5x8yMtcuWe1jFqsU=";
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [ pkgs.openssl ];
          meta.mainProgram = "reindeer";
        };
      in {
        prefetch-npm-deps = pkgs.prefetch-npm-deps;
        buck-npm-cache = pkgs.fetchNpmDeps {
          src = ./web;
          hash = "sha256-yJrPGaSazF06w91mddpNlOyoB6d1cYKXumyaoeWCfeU=";
        };
        buck-rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        buck-cc = pkgs.stdenv.cc;
        buck-binutils = pkgs.binutils;
        buck-node = pkgs.nodejs_24;
        buck-bash = pkgs.bash;
        buck-coreutils = pkgs.coreutils;
        buck-python = pkgs.python3;
        buck-reindeer = buckReindeer;
      } // pkgs.lib.optionalAttrs (pkgs.stdenv.hostPlatform.system == "x86_64-linux") {
        buck2 = pkgs.runCommand "buck2-snapshot-20260926-200119" {
          src = pkgs.fetchurl {
            url = "https://github.com/thoughtpolice/buck2/releases/download/snapshot-20260926-200119/buck2-x86_64-unknown-linux-gnu.zst";
            hash = "sha256-hCos2M7wxjrYKXaQdCouhaWvoK6XM5urBtKJTm2tkfQ=";
          };
          nativeBuildInputs = [ pkgs.zstd ];
        } ''
          mkdir -p "$out/bin"
          zstd --decompress --stdout "$src" > "$out/bin/buck2"
          chmod +x "$out/bin/buck2"
        '';
      });
    };
}
