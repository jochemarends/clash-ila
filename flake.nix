{
  description = "A flake enabling tooling for clash-formal-playground";
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    clash-compiler.url = "github:clash-lang/clash-compiler";
    clash-cores = {
      url = "github:jaschutte/clash-cores/c8f74f21ffffeaa7c765657841269ca8da3babed";
      inputs.clash-compiler.follows = "clash-compiler";
    };
    ecpprog.url = "github:diegodiv/ecpprog";
  };
  outputs = { nixpkgs, ecpprog, flake-utils, clash-compiler, clash-cores, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        # The GHC version you would like to use
        # This has to be be one of the supported versions of clash-compiler
        compiler-version = "ghc9101";

        pkgs = import nixpkgs {
          inherit system;
        };

        # Import the normal and Haskell package set from clash-compiler
        clash-pkgs = ((import clash-compiler.inputs.nixpkgs {
          inherit system;
        }).extend clash-compiler.overlays.${compiler-version})."clashPackages-${compiler-version}";

        # Patch programs to be the correct version we want
        overlay = final: prev: {
          clash-ila = prev.developPackage {
            root = ./ila;
            overrides = _: _: final;
          };
        } // clash-cores.overlays.${system}.default final prev;

        ila-cli = import ./ila-cli/Cargo.nix {
          nixpkgs = nixpkgs;
          pkgs = pkgs;
        };

        # General packages from nixpkgs
        hs-pkgs = clash-pkgs.extend overlay;
      in
      {
        devShells.default = hs-pkgs.shellFor {
          packages = p: [
            p.clash-ila
          ];

          nativeBuildInputs =
            [
              # For interacting with the OrangeCrab board.
              pkgs.yosys
              pkgs.trellis
              pkgs.nextpnr
              pkgs.gnumake

              # Haskell stuff
              hs-pkgs.cabal-install
              hs-pkgs.haskell-language-server

              # Rust subproject
              pkgs.cargo
              pkgs.rustc
              pkgs.clippy

              # Surfer to view VCD
              pkgs.surfer

              # Program the FPGA
              ecpprog.defaultPackage.${system}
            ]
          ;
        };
        packages = {
          clash-ila = hs-pkgs.clash-ila;
          ila-cli = ila-cli;

          default = hs-pkgs.clash-ila;
        };
        apps.default = {
          type = "app";
          program = "${ila-cli.rootCrate.build}/bin/ila-cli";
        };
      });
}
