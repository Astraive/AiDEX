# Portable packaging

AiDEX runs from the drive: a bundle of native binaries is installed into the drive's `app/` directory and started through a small launcher at the drive root. Nothing is installed on, or extracted to, the host.

| Crate | Role |
| --- | --- |
| [`aidex-bundle`](../../crates/bundle/README.md) | Shared contract: the `bundle.json` manifest, its `bundle.sig` signature and the trust store, the `app/current.json` pointer, the update journal, verification, deterministic archives, and atomic installation. The launcher, the packager, and the updater all use it. |
| [`aidex-launcher`](../../crates/launcher/README.md) | The drive-root launcher (`AiDEX.exe`, or `./aidex` → `app/launcher/<target>/aidex-launcher`). Verifies integrity and signature before it runs anything. |
| [`aidex-packager`](../../crates/packager/README.md) | Developer/CI tool: builds release binaries into a bundle with an SBOM, writes the archive and `SHA256SUMS`, generates dev/CI keys, signs and verifies bundles, installs them onto drives, and smoke-tests them. Never shipped in a bundle. |
| [`aidex-updater`](../../crates/updater/README.md) | Signed, staged, self-tested updates with an atomic switch, crash recovery, and rollback (`aidex update …`, daemon `update.*`). |

The packager is its own crate rather than a launcher sub-binary because it drives `cargo` and `git`, reads `cargo metadata`, and writes archives — none of which belongs in the binary that every launch executes. Installation lives in `aidex-bundle` so the updater can stage, verify, and switch bundles with the same code.

## Build a bundle

```sh
cargo run --locked -p aidex-packager -- build --target windows-x64 --out dist
```

`build` runs `cargo build --release --locked --target <triple>` for exactly the shipped binaries (`aidex` from `aidex-cli`, `aidexd`, `aidex-sandbox-exec` from `aidex-sandbox`, and `aidex-launcher`) into `target/packager/`, then:

1. collects license metadata for every crate linked into them (see [Licenses](#licenses));
2. assembles `dist/.aidex-<version>-<target>.staging-<pid>/`, generates the CycloneDX SBOM `docs/sbom.cdx.json` (see [SBOM](#sbom)), writes `bundle.json`, verifies every file against it, and renames it to `dist/aidex-<version>-<target>/` (replacing only a previous packager output);
3. writes the deterministic archive `dist/aidex-<version>-<target>.zip` (Windows) or `.tar.gz` (Linux/macOS) and a copy of the SBOM as `dist/aidex-<version>-<target>.cdx.json`;
4. adds or replaces the archive's and the SBOM's lines in `dist/SHA256SUMS` (`sha256sum -c SHA256SUMS` verifies them).

`build` does not sign. Sign the archive afterwards with `aidex-packager sign --key <file.key> <archive>` (see [Signing](#signing-and-verification)); `--trust-key <file.pub.json>` makes the built binaries trust that key in addition to the release keys.

Each target is built on its own operating system; cross-compiling is not supported or tested. The default triples are `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-musl` (static, so the binaries do not depend on the host's glibc; needs `rustup target add x86_64-unknown-linux-musl` and `musl-tools` for the bundled SQLite), and `aarch64-apple-darwin`; `--triple` overrides it and is recorded in the manifest.

Packaging build settings, applied through the environment of that one `cargo build` (they replace any `RUSTFLAGS` you have set):

| Setting | Why |
| --- | --- |
| `-C target-feature=+crt-static` (Windows) | No dependency on the Visual C++ runtime being installed on the host. |
| `-C link-arg=/Brepro` (Windows) | Deterministic PE timestamps. |
| `--remap-path-prefix` for the workspace and `CARGO_HOME` | No local paths (user names) in panic messages; fewer machine-specific bytes. |
| `CARGO_PROFILE_RELEASE_STRIP=symbols` | Smaller binaries. |
| `SOURCE_DATE_EPOCH` | Commit time (or your `SOURCE_DATE_EPOCH`), also used as `created_at` and as every archive entry's timestamp. |
| `MACOSX_DEPLOYMENT_TARGET=11.0` (macOS) | Runs on macOS 11 and later. |

Optional components: `--desktop <path>` and `--installer <path>` copy a *prebuilt* Tauri app (a file, or a directory such as `AiDEX.app`) under `desktop/` or `installer/` with roles `desktop` / `installer`. The packager never builds the Tauri apps, and nothing in the CLI bundle depends on them. Symbolic links are refused, so macOS app bundles that contain framework links cannot be packaged yet.

## Install a bundle onto a drive

```sh
aidex init --drive H:\                     # once: creates aidex-drive.json and the layout
cargo run -p aidex-packager -- install --bundle dist/aidex-0.1.0-windows-x64.zip --drive H:\
H:\AiDEX.exe doctor
```

`--bundle` takes an archive or an unpacked bundle directory. The signature is checked first: the bundle must be signed by a key the packager trusts (release keys, plus `--trust-key <file.pub.json>`), or `--allow-unsigned` must be given (development only; the launcher then also needs `AIDEX_ALLOW_UNSIGNED=1`). For a developer folder or a test directory that is not removable media, `--init-dev-drive` initializes it first with the removable-media check skipped (the same as `aidex_drive::init_with(.., InitOptions { require_removable: false })`); it is never implied.

Install steps (each is reported to `install_with`'s observer, which can stop the install before it starts):

| Step | What happens |
| --- | --- |
| `stage` | Copy `bundle.json` and exactly the files it lists (directory source) or extract the archive (bounded, links refused) into `app/staging/<id>/` on the drive itself, so the commit is a same-volume rename. |
| `verify` | Parse and validate `bundle.json`; check that the launcher carries the AiDEX launcher marker and that every launcher destination is absent or an AiDEX launcher; apply executable modes; hash every listed file and refuse unlisted files; refuse bundles that need a newer drive schema or host extraction. |
| `commit` | Rename the staged bundle to `app/versions/<version>-<target>/`. If that directory holds a verified copy of the same `bundle.json`, reuse it. If it holds a different build (or a damaged copy), use `<version>-<target>-<first 12 hex of the bundle.json sha256>` instead. Committed directories are never modified. |
| `place_launcher` | Windows targets: copy `bin/aidex-launcher.exe` to `<drive>/AiDEX.exe`. Unix targets: copy `bin/aidex-launcher` to `app/launcher/<target>/aidex-launcher` and write the `<drive>/aidex` entry script. Each file is written as `<name>.aidex-new`, read back, and renamed into place; a replaced launcher is kept as `<name>.previous`. Identical files are left alone. |
| `switch_pointer` | Rewrite `app/current.json` with temporary file + fsync + rename. The target's old entry becomes `previous`. |
| `cleanup` | Remove `app/staging/<id>/`. |

Guarantees:

- Until `switch_pointer` completes, the previous `app/current.json` is byte-for-byte unchanged and the previously active bundle still verifies and runs; an interruption after `place_launcher` leaves a newer launcher in front of the old bundle, which it runs. Retrying the install completes it.
- A drive-root file is replaced only if it is a regular file containing the AiDEX launcher marker. Any other `AiDEX.exe`, `aidex`, or `*.previous` / `*.aidex-new` file makes the install fail before anything is committed.
- A tampered or incomplete source never reaches `app/versions/`; an unreadable `app/current.json` is reported and left untouched.
- Bundles for other targets can be installed from any host (for example, the Linux bundle from Windows); each target has its own pointer entry and launcher.
- The installer does not prune old version directories, and does not take a lock: run one install per drive at a time (staging directories are unique, and the pointer write is atomic last-writer-wins).

## Drive layout

```text
<drive>/
├── aidex-drive.json                  drive manifest (aidex-drive; schema v2)
├── AiDEX.exe                         Windows launcher (copy of bin/aidex-launcher.exe)
├── AiDEX.exe.previous                launcher replaced by the last install, if any
├── aidex                             POSIX sh entry for Linux and macOS
├── app/
│   ├── current.json                  active bundle per target (atomic pointer)
│   ├── versions/
│   │   └── <version>-<target>[-<digest>]/
│   │       ├── bundle.json           bundle manifest
│   │       ├── bundle.sig            Ed25519 signature of bundle.json
│   │       ├── bin/aidex[.exe]                 CLI
│   │       ├── bin/aidexd[.exe]                daemon
│   │       ├── bin/aidex-sandbox-exec[.exe]    sandbox helper (next to aidexd)
│   │       ├── bin/aidex-launcher[.exe]        launcher
│   │       ├── licenses/aidex/LICENSE
│   │       ├── licenses/third-party/<crate>-<version>.txt
│   │       ├── docs/sbom.cdx.json              CycloneDX 1.5 SBOM
│   │       ├── desktop/   installer/           optional prebuilt apps
│   ├── launcher/<target>/aidex-launcher        Unix launchers (selected by ./aidex)
│   ├── update-journal.json           last or in-progress update/rollback (see Updates)
│   ├── update.lock                   held while an update or rollback runs
│   └── staging/<id>/                 installs and updates in progress (removed when done)
└── manifests/
    ├── revoked-keys.json             drive revocation list (optional, additive)
    └── <version>-<target>/           bundle.json, bundle.sig, sbom.cdx.json of every installed update
```

The launcher binary is `aidex-launcher` inside bundles because `AiDEX.exe` and `aidex.exe` are the same file on case-insensitive filesystems (NTFS, exFAT, FAT32, default APFS). `app` is the drive manifest's `app` path; other layout paths come from `aidex-drive.json` as described in [`portable/layouts/README.md`](../layouts/README.md).

## `bundle.json` (schema 1)

A closed schema: every object rejects unknown fields, and a document with another `schema_version` is reported as unsupported (not as malformed). Rust types: `aidex_bundle::{BundleManifest, BundleFile, FileRole, LicenseEntry, StatePaths}`.

```json
{
  "schema_version": 1,
  "product": "aidex",
  "version": "0.1.0",
  "git_commit": "<40 or 64 lowercase hex>",
  "source_dirty": false,
  "target": "windows-x64",
  "rust_target": "x86_64-pc-windows-msvc",
  "rustc": "rustc 1.95.0 (59807616e 2026-04-14)",
  "created_at": "2026-10-10T08:00:00.000Z",
  "min_drive_schema": 2,
  "state": {
    "drive_manifest": "aidex-drive.json",
    "runtime_state": ".aidex/runtime",
    "layout": { "app": "app", "cache": "cache", "private_sessions": "private/sessions", "...": "..." },
    "host_extraction": false
  },
  "files": [
    { "path": "bin/aidex.exe", "sha256": "<64 hex>", "size": 4321, "executable": true, "role": "cli" }
  ],
  "licenses": [
    { "component": "serde", "version": "1.0.229", "spdx": "MIT OR Apache-2.0",
      "source": "registry+https://github.com/rust-lang/crates.io-index",
      "path": "licenses/third-party/serde-1.0.229.txt" }
  ]
}
```

| Field | Meaning |
| --- | --- |
| `schema_version` | Always `1` for this format. |
| `product` | Always `aidex`. |
| `version` | Product version (the `aidex-cli` package version): starts with a digit, `[0-9A-Za-z.+-]`, at most 64 bytes. |
| `git_commit`, `source_dirty` | Source commit and whether tracked or untracked files differed from it (`null` when built outside git). |
| `target` | `windows-x64`, `linux-x64`, `linux-arm64`, `macos-arm64`, or `macos-x64`. |
| `rust_target`, `rustc` | Compiler target triple and version. |
| `created_at` | RFC 3339 UTC; the commit time (or `SOURCE_DATE_EPOCH`) so rebuilds of one commit produce the same manifest. |
| `min_drive_schema` | Lowest `aidex-drive.json` schema the bundle runs on. |
| `state` | Drive-relative state paths the binaries use at build time (`aidex-drive.json`, `.aidex/runtime`, and the default layout); services still resolve real paths from the drive manifest. `host_extraction` is always `false`; launchers refuse a bundle that sets it. |
| `files` | Every file except `bundle.json` and `bundle.sig`, sorted by path: bundle-relative path, lowercase-hex SHA-256, size, executable flag, role. |
| `licenses` | Every crate linked into the binaries, sorted: name, version, declared SPDX expression (or `NOASSERTION`), source (`workspace`, registry, or git URL), and the bundled license text (`null` if the crate ships none). |

Rules enforced by `BundleManifest::validate`:

- Paths are slash-separated, at most 240 bytes and 16 levels, ASCII `[A-Za-z0-9._+@ -]`, with no empty, `.` or `..` segments, no Windows device names (`CON`, `NUL`, `COM1`, ...), and no segment starting or ending with a space or ending with a dot. Paths are unique case-insensitively, and no path is both a file and a directory.
- `bin/aidex-launcher`, `bin/aidex`, `bin/aidexd`, and `bin/aidex-sandbox-exec` (with `.exe` on Windows) must each be listed exactly once with roles `launcher`, `cli`, `daemon`, and `helper`, and be executable.
- Roles and locations: `launcher`, `cli`, `daemon`, `helper` under `bin/`; `runtime` under `bin/` or `runtime/`; `desktop` under `desktop/`; `installer` under `installer/`; `doc` under `licenses/` or `docs/`. Critical roles (verified on every launch): `launcher`, `cli`, `daemon`, `helper`, `runtime`.
- Digests are 64 lowercase hex digits; a license `path` must be a listed `doc` file.

`BundleManifest::to_json` writes pretty JSON with a trailing newline in field order; with `normalize()` applied, equal manifests are byte-identical.

## `app/current.json` (schema 1)

```json
{
  "schema_version": 1,
  "targets": {
    "windows-x64": {
      "version": "0.1.0",
      "dir": "versions/0.1.0-windows-x64",
      "manifest_sha256": "<sha256 of that directory's bundle.json>",
      "previous": { "version": "0.0.9", "dir": "versions/0.0.9-windows-x64", "manifest_sha256": "<64 hex>" }
    }
  }
}
```

- One entry per target, because a drive carries bundles for every host it is used on. Keys must look like target names (`[a-z0-9]+(-[a-z0-9]+)*`); unknown targets are kept when the file is rewritten.
- `dir` is always `versions/<one segment>` (no traversal, no separators, not hidden); `previous` is `null` or the entry that was active before (the updater's rollback candidate). Fields are closed.
- `manifest_sha256` pins the exact `bundle.json`: a swapped or edited manifest is refused even if it matches the files.
- Only `install` / `CurrentPointer::store` write it, by temporary file, fsync, and rename (directory fsync is best effort, and a no-op on Windows).

## Launcher behaviour

1. Locate itself (`current_exe`, canonicalized) and take the nearest ancestor (up to six levels) that contains `aidex-drive.json` as the drive. `AIDEX_DRIVE_ROOT` from the caller is ignored. No autorun, no install, no registry, no host configuration.
2. Open the drive with `DrivePaths::open`, which parses and validates the closed-schema drive manifest and requires schema v2 (a v1 drive is told to run `aidex init`). The launcher must be at the drive root or in `app/launcher/<host target>/`; a copy anywhere else refuses to run.
3. Read `app/current.json` (a regular file, at most 1 MiB) and the entry for the host's target.
4. Check that `app/` and `app/versions/` are real directories and the entry's `dir` stays inside `app/versions/`; read the bundle's `bundle.json` (a regular file, at most 4 MiB) and compare its SHA-256 with `manifest_sha256` **before** parsing it.
5. Validate the manifest; require `target` = host target, `version` = pointer version, `min_drive_schema` ≤ the drive schema, and `host_extraction` = false.
6. Verify `bundle.sig` (see [Signing](#signing-and-verification)) against the keys compiled into this launcher and the drive's `manifests/revoked-keys.json`. A missing signature is refused unless the host environment sets `AIDEX_ALLOW_UNSIGNED=1` (development only; a warning is printed on every launch); a present but invalid, unknown, revoked, or out-of-window signature is always refused.
7. Verify the critical files — `bin/` launcher, CLI, daemon, helper, and runtime files: every path component is a real directory or regular file (no symbolic links or junctions), and size and SHA-256 match. `bin/` must contain nothing that is not listed, so a planted library next to the binaries is refused.
8. Run `bin/aidex` with the caller's arguments unchanged (no arguments: `aidex`'s default), the caller's working directory and standard streams, and:
   - `AIDEX_DRIVE_ROOT` = the drive root (without the Windows `\\?\` prefix),
   - `AIDEX_BUNDLE_DIR`, `AIDEX_BUNDLE_VERSION`, `AIDEX_BUNDLE_TARGET`, `AIDEX_LAUNCHER_VERSION`,
   - `PATH` = the bundle's `bin/` followed by the inherited `PATH`.

   On Linux and macOS the launcher `exec`s the CLI (same process); on Windows it waits and exits with the CLI's exit code, ignoring Ctrl+C itself so the CLI handles it.

Reserved options (first argument only; anything else, including other `--launcher-*` spellings being rejected, goes to `aidex`): `--launcher-info` verifies as above and prints a one-line JSON report (launcher version, marker, drive root, target, bundle version/dir/path/digest/commit, scope, number of verified files, previous) without running the CLI; `--launcher-verify` does the same while verifying **every** listed file and refusing any unlisted file in the bundle.

Exit codes (on stderr: `AiDEX launcher: <message>` and a `hint:` line):

| Code | Meaning |
| --- | --- |
| CLI's code | The CLI ran; its exit status is passed through. |
| 1 | Unexpected I/O error or the CLI could not be started. |
| 2 | Unknown `--launcher-*` option. |
| 4 | Not on an initialized AiDEX drive, misplaced launcher, or an invalid / v1 drive manifest. |
| 8 | No bundle installed (`app/current.json` missing) or none for this host's target. |
| 10 | Incompatible: wrong-target bundle, newer pointer or manifest schema, newer drive schema, host extraction requested, or an unnamed host platform. |
| 11 | Integrity or signature failure: hash or size mismatch, missing file, link, unlisted file in `bin/`, pointer escaping `app/versions/`, manifest not matching its pin, version disagreement, malformed pointer/manifest, unsigned bundle (without the opt-in), malformed `bundle.sig`, unknown, revoked, or out-of-window key, bad signature, or an unreadable revocation list. Nothing was run. |
| 126 | The CLI is not executable (for example a `noexec` or `showexec` mount). |

If `app/update-journal.json` names an unfinished update, the launcher prints a note, reports it as `pending_update` in `--launcher-info`, and runs what the pointer selects (always a complete, verified old or new version); the daemon finishes or rolls back the update when it starts.

The launcher writes nothing — not to the host and not to the drive — and extracts nothing (bundles declare `host_extraction: false`), so there is no host residue for the cleanup manager (#28) to track. The Unix `aidex` script maps `uname -s`/`uname -m` to a target and `exec`s `app/launcher/<target>/aidex-launcher`, exiting 126/127 with a message when that launcher is missing or not executable.

## Licenses

The inventory is the normal-dependency closure of the four shipped packages for the target platform, from `cargo metadata --locked --filter-platform <triple>`; build and dev dependencies are not linked and are excluded, proc-macro crates are included. Each crate's declared SPDX expression is recorded, and its `LICENSE*`, `LICENCE*`, `COPYING*`, `NOTICE*`, `COPYRIGHT*`, and `UNLICENSE*` files (plus `license-file`) are concatenated into `licenses/third-party/<name>-<version>.txt`. Workspace crates are covered by the repository's MIT `LICENSE`, shipped as `licenses/aidex/LICENSE`. Crates with no license file in their package are listed with `path: null` and reported by `build`. The same closure is described, with versions, purls, and hashes, by the SBOM below.

## SBOM

Every bundle carries a CycloneDX 1.5 JSON SBOM at `docs/sbom.cdx.json` (listed in `bundle.json` with role `doc`, so it is hashed and covered by the signature); `build` also writes it next to the archive as `aidex-<version>-<target>.cdx.json` with a `SHA256SUMS` line, and every update copies it to `manifests/<version>-<target>/sbom.cdx.json` on the drive.

- `metadata.component`: the bundle (`type: application`, `name: aidex`, the version, MIT, properties `aidex:target`, `aidex:rust_target`, `aidex:git_commit`, `aidex:source_dirty`, `aidex:rustc`); `metadata.timestamp` is `created_at`; `metadata.tools` names `aidex-packager`.
- `components`: every crate linked into the shipped binaries (`type: library`) with name, version, purl (`pkg:cargo/<name>@<version>` for crates.io; `repository_url`/`vcs_url` qualifiers for other registries and git; none for unpublished workspace crates, whose `bom-ref` is `workspace:<name>@<version>`), the declared SPDX expression (Cargo's `MIT/Apache-2.0` becomes `MIT OR Apache-2.0`), the `Cargo.lock` SHA-256 of the `.crate` archive where recorded, and `aidex:source`; plus one `type: file` component per critical binary (`bin/...`) with its SHA-256.
- `dependencies`: the linked dependency graph, rooted at the bundle.
- Deterministic: sorted components, no random serial number, timestamp = `created_at`.

`aidex_updater::sbom::Sbom::validate` checks the format, spec version, timestamp, subject, unique `bom-ref`s, purls agreeing with name and version, hash algorithms and digest lengths, license entries, and dependency references; `aidex doctor` and `aidex update verify` report whether the installed bundle's SBOM matches its digest and validates.

## Signing and verification

A bundle is signed by a detached Ed25519 signature over its `bundle.json`; because `bundle.json` pins the size and SHA-256 of every file (including the SBOM), the signature authenticates the whole bundle.

`bundle.sig` (schema 1, closed), next to `bundle.json` and never listed in it:

```json
{
  "schema_version": 1,
  "algorithm": "ed25519",
  "key_id": "<16 hex: first 8 bytes of SHA-256(public key)>",
  "manifest_sha256": "<sha256 of the signed bundle.json>",
  "signature": "<128 hex>"
}
```

The signed message is the ASCII context `AIDEX-BUNDLE-SIG-V1` and a newline, followed by the exact bytes of `bundle.json` (no re-serialization, so no canonical-JSON rules are involved); verification uses `verify_strict`.

**Trust.** Binaries carry their trust store: the release public keys in `crates/bundle/src/signature/release-keys.json` (with `not_before`/`not_after` and compiled-in revocations), plus keys baked in at compile time with `aidex-packager build --trust-key` (`AIDEX_BUILD_TRUSTED_KEYS`; CI's ephemeral key, or a developer's key). Test builds additionally honour `AIDEX_DEV_TRUSTED_KEYS` at run time (the `dev-trust` feature is enabled only through dev-dependencies; release bundles never read it). A key's validity window is checked against the bundle's signed `created_at`; a key that may be compromised must therefore be **revoked**, not just expired, because its holder could sign a manifest with an earlier `created_at`. Revocations come from the compiled-in list and the drive's `manifests/revoked-keys.json` (`{"schema_version":1,"revoked":[{"key_id":"…","revoked_at":"…","reason":"…"}]}`), are additive, and win over trust.

**Unsigned bundles** are refused by the launcher, `aidex update`, and `aidex-packager install` by default. The development opt-ins are `AIDEX_ALLOW_UNSIGNED=1` (launcher, `aidex doctor`, `aidex update verify`), `--allow-unsigned` (`aidex update apply|check|rollback|verify`, `aidex-packager install|smoke|verify`); each prints a warning, and the updater records `update.verified { unsigned_allowed: true }`. No opt-in ever accepts a present but invalid or untrusted signature.

### Signing keys

- **Development and CI keys** are made with `aidex-packager keygen --name <name> --out <dir outside the repository>`: `<name>.key` (secret, 0600 on Unix, never overwritten) and `<name>.pub.json`. `*.key` is git-ignored; never commit, upload, or share the secret file. CI (`.github/workflows/package.yml`) generates an ephemeral key in each job, bakes its public half into that job's binaries, signs the archive, verifies it, smoke-tests it, and uploads only the public key. Such bundles are trusted only by binaries built in the same job.
- **The release key** does not exist in this repository or in any workspace that pull requests can reach. The project generates it offline (or in an HSM), and stores the secret only in a protected signing environment — for GitHub, an Actions environment restricted to release tags with required reviewers, or an offline signing machine. Its public key and validity window are added to `release-keys.json` by a reviewed pull request; release builds are then made **without** `--trust-key`, so they trust release keys only, and signed in the protected environment with `aidex-packager sign`. No release key is provisioned yet: `release-keys.json` is empty, so every build today trusts only the keys it was built with.
- **Rotation**: add the new key (with `not_before`) to `release-keys.json` and ship a release, signed by the *old* key, whose binaries trust both; sign the next releases with the new key; give the old key a `not_after` once drives have moved on. Because the launcher that verifies a bundle is the one already on the drive, an update must always be signed by a key the *currently installed* version trusts.
- **Compromise**: add the key id to the compiled-in `revoked` list and ship a release signed by another trusted key; until drives update, users can add the id to `manifests/revoked-keys.json` (it is honoured immediately by the launcher, `aidex update`, and `aidex doctor`).

### Verify a release (users)

1. Check the download: `sha256sum -c SHA256SUMS --ignore-missing` (macOS: `shasum -a 256 -c SHA256SUMS`), Windows PowerShell: `Get-FileHash aidex-<version>-windows-x64.zip -Algorithm SHA256` and compare with the line in `SHA256SUMS`. This only proves the file arrived intact.
2. Check the signature with an AiDEX you already trust: `aidex update check <archive>` verifies `bundle.sig` against the keys built into your installed AiDEX (and your drive's revocation list), every file hash, the target, and the version rules, and prints the signer's key id — compare it with the key id the project publishes. `aidex-packager verify [--trust-key <file.pub.json>] <archive>` does the same from a source checkout.
3. On a drive, `aidex update verify` (or `AiDEX.exe --launcher-verify`) re-verifies the installed version: signature, every file, and the SBOM; `aidex doctor` reports version, signer key id, verification, and SBOM status.

## Updates

```sh
aidex update check   <archive|bundle dir|release folder>     # verify only
aidex update apply   <archive|bundle dir|release folder> [--allow-downgrade] [--allow-unsigned]
aidex update status                                          # active, previous, integrity, last operation
aidex update rollback                                        # current ↔ previous
aidex update verify                                          # installed bundle: signature, every file, SBOM
```

`check`, `apply`, `rollback`, and `status` run in the daemon (`update.*` methods); `verify` runs locally. An update moves through `staging` (bounded extraction into `app/staging/<id>/`) → `verified` (signature, every file, target, drive schema, version rules) → `self_test` (the staged `aidex --version`, `aidexd --version`, `aidex doctor --json`) → `switching` (rename into `app/versions/`, atomic replacement of `app/current.json`; the old bundle becomes `previous`) → `committed` (drive-root launchers from the new bundle, provenance copied to `manifests/`, versions other than current and previous pruned). Each step is recorded in `app/update-journal.json` first; after a crash or power loss the daemon, when it starts, rolls the operation back to the previous pointer or — if the switch had happened and the new bundle verifies — finishes it. Neither the daemon nor the CLI replaces its own running binaries: the new version takes effect the next time AiDEX is started, and `aidex update apply` says so.

Versions follow SemVer precedence: older packages are refused (`downgrade`) unless `--allow-downgrade` is given **and** the package is exactly the recorded previous bundle. Every refusal (bad, unknown, or revoked signature, tampered or missing file, wrong platform, downgrade, incompatible drive schema, unsafe or oversized archive, failed self-test, checksum mismatch) leaves the drive unchanged; integrity and signature refusals exit 11, version and platform refusals exit 10. Details: [`crates/updater/README.md`](../../crates/updater/README.md).

## Verification and its limits

Verification detects corruption, partial copies, accidental edits, wrong-platform bundles, and files planted next to the binaries, and signatures detect bundles and update packages that were not produced by a trusted signer; both fail closed. The trust anchor is compiled into the launcher and CLI **on the drive**, so signatures do not defend against someone who can replace `AiDEX.exe` or `aidex` itself; they do stop a modified bundle, manifest, or pointer, and an untrusted update package, from being run or installed by an intact AiDEX. Other known gaps:

- Files are hashed and then executed by path, so a process that rewrites the drive between the check and `exec` is not detected.
- Windows resolves a launcher's DLL imports before `main` runs, so a DLL planted next to `AiDEX.exe` at the drive root could be loaded before any check (the bundle's own `bin/` is checked).
- Binaries are unsigned. Windows SmartScreen and Mark-of-the-Web may warn for archives downloaded from the internet; macOS Gatekeeper quarantines downloaded archives (`xattr -dr com.apple.quarantine <dir>` after verifying `SHA256SUMS`).

## Reproducibility

Deterministic by construction: build inputs (`--locked`, pinned toolchain channel, fixed flags), file order, the manifest bytes, `created_at` (commit time), and archive metadata (sorted entries, entry times = `created_at`, modes 0755/0644, owner 0:0, empty user/group names, gzip header without name or time, fixed compression level). Rebuilding one commit on one machine with the same toolchain reproduces the archive in the tests and local runs; bit-for-bit identical binaries across machines or toolchain patch releases are **not** verified, and a dirty tree (`source_dirty: true`) is not reproducible by definition. `SHA256SUMS` describes what was built, not that anyone else rebuilt it.

## Smoke test

```sh
cargo run -p aidex-packager -- --json smoke --archive dist/aidex-0.1.0-windows-x64.zip --work <empty dir>
```

`smoke` checks the archive against `SHA256SUMS` (when present next to it), initializes `<work>/drive` as a developer drive, installs the archive, and runs the drive-root entry (`AiDEX.exe` or `./aidex`) with `--launcher-verify`, `--version`, `doctor --json`, `daemon start`, `daemon status --json`, and `daemon stop`. It requires success for each, except that `doctor` may exit 1 (it requires Oh My Pi and a removable drive, which CI folders are not) as long as it prints a well-formed report from the bundled CLI version. It then fails if new entries appeared in the home directory, the per-user config/data/state/cache roots (`%APPDATA%` and `%LOCALAPPDATA%` on Windows, XDG directories on Linux, `~/Library/{Application Support,Caches,Preferences,Logs}` on macOS), or AiDEX-named entries in the system temp directory, or if the drive root holds anything but the manifest, `.aidex`, launcher entries, and layout directories. On macOS, background services' own `com.apple.*` state in Preferences, Application Support, and Caches is not counted ([details](../../crates/packager/README.md#limitations)). The host check lists those directories' top level only; deeper writes inside existing directories (for example PowerShell's module cache when `aidexd` is started through `Start-Process`) are not detected.

## Platform support

| Target | Built by CI (`package.yml`) | Smoke-tested | Status |
| --- | --- | --- | --- |
| `windows-x64` | yes | CI (`windows-latest`) and locally on Windows 11 | Supported for the CLI bundle. |
| `linux-x64` (musl) | yes | CI (`ubuntu-latest`) | Supported for the CLI bundle; on FAT32 drives mounted with `showexec` (the udisks default) nothing without `.exe` is executable, so remount with exec or use exFAT/ext4. |
| `macos-arm64` | yes | CI (`macos-latest`) | CI-tested only; no `AiDEX.app` (use `./aidex` from Terminal). |
| `linux-arm64` | no | no | Not supported: the target name exists, but no bundle is built or tested. |
| `macos-x64` | no | no | Not supported: the target name exists, but no bundle is built or tested. |

A macOS `AiDEX.app` entry (a signed and notarized app bundle that opens Terminal on `./aidex`) is future work; it needs signing infrastructure that does not exist yet.

## Not included yet

- A provisioned release key (see [Signing keys](#signing-keys)), provenance attestations (SLSA/in-toto), OS code signing (Authenticode, notarization), and an online update feed or channels; updates are offline/manual packages.
- Bun and Oh My Pi: `aidex chat` and `aidex run` still need Oh My Pi installed on the host; the native agent and local model runtimes (#15) are not bundled.
- Building the Tauri desktop and installer apps.
