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
    url = "github:oxalica/rust-overlay/c62195b3d6e1bb11e0c2fb2a494117d3b55d410f";
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
        web = pkgs.mkShell {
          packages = [ pkgs.nodejs_24 ];
          PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD = "1";
          PLAYWRIGHT_BROWSERS_PATH = "${pkgs.playwright-driver.browsers-chromium}";
        };
      });

      packages = forSystems (pkgs: {
        prefetch-npm-deps = pkgs.prefetch-npm-deps;
        # Browser downloads are Nix inputs, independent of npm install scripts.
        web-chromium = pkgs.playwright-driver.browsers-chromium;
        buck-npm-cache = pkgs.fetchNpmDeps {
          src = ./web;
          hash = "sha256-gXYGez5cJIcLl6KoAiG3bH1wrEmyf+kax+uoMtj+99g=";
        };
        buck-rust = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        buck-cc = pkgs.stdenv.cc;
        buck-binutils = pkgs.binutils;
        buck-node = pkgs.nodejs_24;
        buck-bash = pkgs.bash;
        buck-coreutils = pkgs.coreutils;
        buck-tar = pkgs.gnutar;
        buck-gzip = pkgs.gzip;
        buck-python = pkgs.python3;
      } // pkgs.lib.optionalAttrs (pkgs.stdenv.hostPlatform.system == "x86_64-linux") {
        buck-reindeer = pkgs.runCommand "reindeer-2026.09.14.00" {
          src = pkgs.fetchurl {
            url = "https://github.com/facebookincubator/reindeer/releases/download/v2026.09.14.00/reindeer-x86_64-unknown-linux-musl.zst";
            hash = "sha256-YWqPwwLD2yuJ5yKGz3pTlpkRY7CI8BWHyb+c/css2qw=";
          };
          nativeBuildInputs = [ pkgs.zstd ];
        } ''
          mkdir -p "$out/bin"
          zstd --decompress --stdout "$src" > "$out/bin/reindeer"
          chmod +x "$out/bin/reindeer"
        '';
        buck2 = pkgs.runCommand "buck2-snapshot-20260926-200119" {
          src = pkgs.fetchurl {
            url = "https://github.com/thoughtpolice/buck2/releases/download/snapshot-20260926-200119/buck2-x86_64-unknown-linux-gnu.zst";
            hash = "sha256-hCos2M7wxjrYKXaQdCouhaWvoK6XM5urBtKJTm2tkfQ=";
          };
          # The GNU release needs the pinned loader and runtime library paths.
          nativeBuildInputs = [ pkgs.zstd pkgs.autoPatchelfHook ];
          buildInputs = [ pkgs.stdenv.cc.libc pkgs.stdenv.cc.cc.lib ];
        } ''
          mkdir -p "$out/bin"
          zstd --decompress --stdout "$src" > "$out/bin/buck2"
          chmod +x "$out/bin/buck2"
          autoPatchelf "$out"
        '';
      });
    };
}
