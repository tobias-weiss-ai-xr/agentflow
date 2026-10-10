{
  description = "agentflow (af) — parallel LLM task execution on isolated git worktrees";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: builtins.listToAttrs (map (s: { name = s; value = f s; }) systems);
    in
    {
      packages = forAll (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          cargoToml = pkgs.lib.importTOML ./Cargo.toml;
          # Single source of truth: derive the crate version once from
          # Cargo.toml so the package, static build and image tag can never
          # drift from the crate version.
          version = cargoToml.package.version;
          buildAf = pkgs: pkgs.rustPlatform.buildRustPackage {
            pname = "agentflow";
            version = version;
            src = self;
            cargoLock.lockFile = ./Cargo.lock;
            # CI owns the 340-test suite; nix builds stay fast and green.
            doCheck = false;
            meta.mainProgram = "af";
          };
          af = buildAf pkgs;
          # Fully static musl binary — the portable one. Nix wires the
          # crt-static flags correctly (hand-rolled RUSTFLAGS segfaulted).
          afStatic = buildAf pkgs.pkgsStatic;
          # Campaign-runner image: af plus the toolchain its tasks' gates
          # need (cargo gates, git worktrees, bash gates, TLS for the API).
          image = pkgs.dockerTools.buildImage {
            name = "agentflow";
            tag = version;
            copyToRoot = pkgs.buildEnv {
              name = "af-image-root";
              paths = [ af pkgs.git pkgs.bash pkgs.cargo pkgs.rustc pkgs.stdenv.cc pkgs.cacert
                        # gates are arbitrary shell — give them a POSIX-ish base
                        pkgs.coreutils pkgs.gnugrep pkgs.gnused pkgs.findutils ];
              pathsToLink = [ "/bin" "/etc" ];
            };
            config = {
              # ENTRYPOINT (not Cmd): `docker run agentflow --version` then
              # appends args to af instead of replacing it.
              Entrypoint = [ "${af}/bin/af" ];
              Env = [
                "PATH=/bin"
                "SSL_CERT_FILE=${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
              ];
            };
          };
        in
        { inherit af afStatic image; default = af; static = afStatic; });
    };
}
