# aidex-packager

Developer and CI tool for portable bundles. It is never shipped inside a bundle.

```sh
cargo run --locked -p aidex-packager -- build --target windows-x64 --out dist
cargo run -p aidex-packager -- install --bundle dist/aidex-0.1.0-windows-x64.zip --drive H:\
cargo run -p aidex-packager -- --json smoke --archive dist/aidex-0.1.0-windows-x64.zip --work <empty dir>
cargo run -p aidex-packager -- keygen --name dev --out ~/.aidex-dev-keys
cargo run -p aidex-packager -- sign --key ~/.aidex-dev-keys/dev.key dist/aidex-0.1.0-windows-x64.zip
cargo run -p aidex-packager -- verify --trust-key ~/.aidex-dev-keys/dev.pub.json dist/aidex-0.1.0-windows-x64.zip
```

- `build --target <t> [--out dist] [--triple <triple>] [--target-dir <dir>] [--manifest-path <Cargo.toml>] [--desktop <path>] [--installer <path>] [--trust-key <file.pub.json>]...`: `cargo build --release --locked` of `aidex`, `aidexd`, `aidex-sandbox-exec`, and `aidex-launcher` with the packaging flags (static CRT and `/Brepro` on Windows, remapped source paths, stripped symbols, `SOURCE_DATE_EPOCH`), license collection from `cargo metadata`, a CycloneDX 1.5 SBOM (`docs/sbom.cdx.json` in the bundle, role `doc`, also written as `dist/aidex-<version>-<target>.cdx.json`), `bundle.json`, full verification, a deterministic `.zip`/`.tar.gz`, and `SHA256SUMS` entries for the archive and the SBOM. `--trust-key` bakes extra public keys into the binaries (`AIDEX_BUILD_TRUSTED_KEYS`, compile time; without it the variable is removed from the build environment). Reports file counts, sizes, digests, SBOM components, baked keys, and timings. The bundle is **not** signed by `build`.
- `keygen --out <dir> [--name dev]`: a development/CI Ed25519 key pair, `<name>.key` (secret, 0600 on Unix, never overwritten) and `<name>.pub.json`. Never a release key.
- `sign --key <file.key> <archive|dir>`: writes `bundle.sig` (a directory) or rewrites the archive with it deterministically and updates its `SHA256SUMS` line.
- `verify [--trust-key <file.pub.json>]... [--allow-unsigned] <archive|dir>`: `SHA256SUMS` (when next to the archive), every file, the signature against the built-in keys plus `--trust-key`, and the SBOM.
- `install --bundle <archive|dir> --drive <root> [--init-dev-drive] [--trust-key F]... [--allow-unsigned]`: `aidex_bundle::install_signed`; unsigned bundles only with `--allow-unsigned`. `--init-dev-drive` first initializes a non-removable developer folder (explicit opt-in only).
- `smoke --archive <file> --work <dir> [--timeout <s>] [--trust-key F]... [--allow-unsigned]`: checks `SHA256SUMS`, installs into `<work>/drive` with the signature policy, runs the drive-root entry (`--launcher-verify`, `--version`, `doctor --json`, `update verify`, `daemon start`, `daemon status --json`, `daemon stop`), requires the bundled CLI to report the installed signer and a valid SBOM, and fails on host residue in the watched per-user directories or unexpected names at the drive root.
- `--json` prints one JSON report on stdout; errors go to stderr (`{"error":{"message":...}}` with `--json`) with exit code 1.

Why a separate crate: the packager drives `cargo` and `git`, parses `cargo metadata`, and writes archives — dependencies and code that the launcher, which runs on every start, must not carry. The install logic it calls lives in `aidex-bundle`, shared with the updater.

See [`portable/packaging/README.md`](../../portable/packaging/README.md) for the build settings, layout, schemas, reproducibility notes, and the platform matrix.

## Limitations

- Builds the host's own targets only; cross-compilation is untested.
- Does not build or sign the Tauri apps; `--desktop`/`--installer` copy prebuilt artifacts and refuse symbolic links.
- No provenance attestation (SLSA/in-toto) and no signing of the Tauri apps or OS-level code signing (Authenticode, notarization).
- The smoke's host-residue check lists the top level of the watched directories only; it does not detect writes inside existing directories or changes to existing entries. On macOS, three OS-managed directories skip new state in Apple's `com.apple.` namespace (below); every other new entry, including unrelated and AiDEX names, is audited. File types are captured with the snapshot without following symlinks; unreadable types are never excluded. The audit identifies new entries under these rules; it does not attribute every host write to a process or prove the absence of host writes.

macOS services write their own state into the per-user Library on their own schedule, under Apple's reserved `com.apple.` reverse-DNS namespace. Four package smoke runs on `main` failed on GitHub's macOS runners within three days, each on Apple entries that no earlier exact-name exception covered (Siri, Apple Intelligence reporting, remote management, managed app distribution), so an exact-name list cannot keep up. The Windows and Linux jobs have never reported OS-owned entries. These directories skip that namespace with the type the OS keeps there:

| Watched directory               | Skipped new entries                            |
| ------------------------------- | ---------------------------------------------- |
| `~/Library/Preferences`         | Regular files named `com.apple.<domain>.plist` |
| `~/Library/Application Support` | Directories named `com.apple.<domain>`         |
| `~/Library/Caches`              | Directories named `com.apple.<domain>`         |

`<domain>` is one or more non-empty, dot-separated labels of ASCII letters, digits, `-`, and `_`; the `com.apple.` prefix is case-sensitive. Still audited: any name containing `aidex` in any case (including AiDEX's `dev.aidex.*` identifiers and `com.apple.aidex.plist`), every name outside the namespace, other types and symlinks at Apple names, and Apple names in the home directory, `~/Library/Logs`, or under the other directory policy. The Windows and Linux directories have no exceptions. Skipped entries count as host-controlled state ([spec §41](../../docs/spec.md)) even if an AiDEX action led an Apple service or tool to write them, and their contents are outside this top-level audit's coverage.
