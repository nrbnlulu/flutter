# rlutter: a standalone CLI for the Flutter Rust shell

Status: implemented for Linux (debug + release) and Android debug. Android release, and everything that needs a
phone, is not verified yet (see Progress and the 2026-10-19 log entries for exactly what).

## Progress

Legend: [ ] todo, [~] in progress, [x] done, [!] blocked / needs decision.

- [x] Phase 0: spike (Linux debug + release AOT verified; Android checks are deferred to Phases 4 and 5, see log)
- [x] Phase 1: scaffold, SDK, plugins, create, clean (34 unit tests; `rlutter create` + `cargo build` of the runner verified)
- [x] Phase 2: Linux build (debug + release verified on a generated app; plugin and native-lib paths verified with local fixture packages)
- [x] Phase 3: Linux run (debug hot reload/restart/DevTools and release verified)
- [~] Phase 4: Android build (debug APK built and inspected; release blocked by a stale local engine, see log)
- [~] Phase 5: Android run (code + parsing unit tests only; never run against a device, none was attached)
- [x] Phase 6: cut over (tool diff empty, verified end to end against the pristine tool; `android_shell_app` kept as the engine-dev scaffold)

### Log

- 2026-10-19: started Phase 0.
- 2026-10-19: Phase 0 results (Linux, debug, app created with `--platforms=android` only, so no `linux/` dir):
  - `flutter build bundle --debug --local-engine=<synthetic>/engine/src/out/host_debug --local-engine-host=<same>`
    works with a layout made only of symlinks. Required entries: `dart-sdk`, `flutter_patched_sdk`,
    `frontend_server_aot.dart.snapshot`, `gen_snapshot`, `icudtl.dat`, `libflutter_rust_engine.so`, and
    **`gen/flutter/lib/snapshot/`** (debug bundle copies `vm_isolate_snapshot.bin` / `isolate_snapshot.bin`
    from there; missing it fails `copy_flutter_bundle`). The kernel compiled with no SDK-hash error.
  - The runner prints the stock `The Dart VM service is listening on http://127.0.0.1:<port>/<token>/` line.
  - `flutter attach --debug-url <uri>` with **no `-d` refuses**: "not supported by this project" (no `linux/`).
    **`-d linux` works** with no `linux/` dir. Verified: hot reload applied a real edit
    ("Reloaded 1 of 1040 libraries"), hot restart ("Restarted application in 722ms"), DevTools URL printed.
    So rlutter must always pass `-d linux` for the Linux host. No stub dir or own reload driver needed.
  - `q` in attach only detaches: the runner keeps running. rlutter owns the runner lifecycle and must kill it.
  - Not verified: anything on Android. No device is attached on this machine (Phase 4/5; stock
    `AndroidDevice` matching by serial is expected to work).
- 2026-10-19: Phase 0 addendum, release AOT on Linux through the layout works:
  `flutter --local-engine=<host_release layout> --local-engine-host=<same> assemble --output=<dir> -dTargetPlatform=linux-x64 -dBuildMode=release -dTargetFile=<t> copy_flutter_bundle aot_elf_release`.
  - Assets land directly in `<dir>` (no `flutter_assets/` subdirectory); release has no kernel_blob.
  - `app.so` is an x86-64 ELF at `<project>/.dart_tool/flutter_build/<id>/app.so`, where `<id>` is the content
    of `<dir>/.last_build_id`. rlutter reads that file to find it.
  - The stock `*_bundle_linux-x64_assets` targets can't be used: they depend on `unpack_linux` (GTK embedder files).
    The generic `copy_flutter_bundle` + `aot_elf_release` pair is what the old `RustAotBundle` target amounted to.
- 2026-10-19: Phase 1 implemented in `engine/src/flutter/shell/platform/rust/rlutter/`.
  Deviations from this plan:
  - **Standalone crate** at `rust/rlutter` with its own `[workspace]`, not `crates/rlutter` in the engine workspace:
    that workspace's Cargo.toml/Cargo.lock are copied into every app's SDK dir, so clap & co. must not be in them.
  - `rlutter clean` does **not** run `flutter clean` by default (it deletes `.dart_tool`, including the SDK we keep
    to avoid re-downloading). `--all` does.
  - `lib/<platform>-<mode>` is a symlink to `engine/src/out/<variant>` (one real directory of links, not two).
  - `rlutter create` also strips the `platform: linux` entry from `.metadata` when it removes the stub `linux/`
    dir, so a later `flutter create .` doesn't resurrect it.
  Known issue until Phase 6: the old tool's `pub get` hook (`refreshRustPlugins`) still fires for `rust-shell: true`
  projects, rewriting the runner with the v4 templates before rlutter rewrites them as v5. Harmless (rlutter
  runs second) but noisy; it also creates the legacy `sdk/lib/debug` dirs.
  Not yet verified: the BETA download path of `sdk::ensure_host` (only the local-checkout path was exercised).

## Goal

Move every Rust-shell-specific piece out of `packages/flutter_tools` into a
standalone Rust CLI, `rlutter`. The CLI owns the native side (cargo, Gradle,
adb, devices, packaging) and **delegates the Dart side to the stock `flutter`
tool** (`create`, `pub get`, `assemble`, `attach`) instead of reimplementing
it. When we're done, this branch has zero Rust-shell diffs in `flutter_tools`.

Scope for v1:

- Hosts: Linux x64.
- Targets: Linux x64 and Android **arm64 only**.
- Build modes: debug and release. Profile is reserved in the CLI but rejected.
- Commands: `create`, `build`, `run`, `devices`, `clean`.

Non-goals for v1 (the design must not block them):

- All Android ABIs (`x86_64` for emulators, `armeabi-v7a`, `riscv64`). Every
  Android step works on a list of ABIs from day one, so adding one means
  shipping artifacts, not changing code.
- Profile mode.
- `AAssetManager`-backed asset loading (v1 extracts assets to the files dir).
- App bundles (`.aab`), release signing configuration, Windows/macOS hosts.

## Current state (what we are moving)

| Today (in `flutter_tools`) | Responsibility | Moves to |
|---|---|---|
| `lib/src/rust/build_rust.dart` | Bundle build with the engine-matched compiler, `cargo build`, staging `data/` + `libs/` next to the runner | `rlutter::build::linux` |
| `lib/src/rust/rust_plugins.dart` | SDK setup (local checkout symlinks or BETA download), Rust plugin discovery, generated Cargo.toml section, `flutter_plugins.rs`, runner-version regeneration | `rlutter::sdk`, `rlutter::plugins` |
| `lib/src/rust/native_libraries.dart` | frb-style `cdylib` discovery + build + staging | `rlutter::plugins::native_libs` |
| `lib/src/rust/rust_device.dart` | Linux pseudo-device for `flutter run -d rust` | `rlutter::device::linux` |
| `lib/src/commands/build_rust.dart` | `flutter build rust` | `rlutter build linux` |
| `create.dart`, `create_base.dart`, `templates/rust_shell/**`, `template_manifest.json` | `flutter create --shell=rust` | `rlutter create` |
| `project.dart` hook | `refreshRustPlugins` during `pub get` | `rlutter` runs it before each build |
| `desktop_device.dart`, `bundle_builder.dart`, `flutter_device_manager.dart`, `flutter_manifest.dart`, `cache.dart` | Small extension points for the above | Reverted |

Android today has no app-level path: `engine/src/flutter/shell/platform/rust/android_shell_app`
is a scaffold with a hardcoded `dev.flutter.rustshell`, and
`flutter-shell-android-runner` reads assets from an adb-pushed
`/data/data/dev.flutter.rustshell/files`.

## Key enabler: engine-matched compilation via `--local-engine`

The Rust-shell engine only accepts kernels from a frontend_server built from the
same tree ("Invalid SDK hash" otherwise). `build_rust.dart` solves this by
overriding `Artifacts` in-process. We can't do that from outside the tool, but
stock flutter accepts an absolute local-engine path whose parents are named
`out/` and `src/` (`runner/local_engine.dart:_findEngineSourceByBuildPath`):

```
--local-engine=<abs>/src/out/<variant> --local-engine-host=<abs>/src/out/host_<mode>
```

So `rlutter` lays out an engine-shaped tree and passes those flags on **every**
`flutter` invocation it makes:

```
.dart_tool/flutter_rs/sdk/
  Cargo.toml, Cargo.lock            # workspace manifest for the shell crates
  crates/ -> …                      # flutter-plugin-sdk, flutter-shell-winit, …
  third_party/ -> …
  lib/<platform>-<mode>/            # what runner-rs/build.rs links (see below)
  engine/src/out/
    host_debug/                     # dart-sdk, flutter_patched_sdk, frontend_server_aot, gen_snapshot, icudtl.dat, libflutter_rust_engine.so
    host_release/
    android_debug_arm64/            # flutter_patched_sdk, icudtl.dat, libflutter_rust_engine.so
    android_release_arm64/          # + clang_x64/gen_snapshot
```

- **Local checkout** (`$FLUTTER_ROOT/engine/src/out` exists): symlink the real
  variants. Name mapping is a table, e.g. `android_debug_arm64 ->
  android_debug_unopt_arm64` (the only debug Android build we have locally).
- **Downloaded SDK**: the release archive unpacks into this exact layout.
  CI must publish Android arm64 artifacts alongside the existing
  `flutter-rust-linux-x64.tar.gz` (one archive per host, containing every target
  variant, or one per target; decide in Phase 4).
- `lib/<platform>-<mode>/` replaces today's `lib/<profile>/`. `runner-rs/build.rs`
  picks the directory from `CARGO_CFG_TARGET_OS`/`CARGO_CFG_TARGET_ARCH` + `PROFILE`.
  This is a runner-version bump (v4 → v5), handled by the existing regeneration logic.

## Command surface

```
rlutter create [--rust-platforms=linux,android] [--org <org>] [--project-name <name>] <dir> [-- <flutter create args…>]
rlutter build linux   [--debug|--release] [-t lib/main.dart]
rlutter build android [--debug|--release] [-t lib/main.dart] [--target-platform android-arm64[,…]]
rlutter run           [-d <device-id>] [--debug|--release] [-t lib/main.dart] [--no-devtools] [-- <app args…>]
rlutter devices
rlutter clean
```

Global: `--verbose`, `--flutter-root <path>` (default: `$FLUTTER_ROOT`, else
the release-managed SDK under `~/.cache/rlutter`), `--local-engine-src <path>`
(override auto-detection of a local checkout).

### Build modes

```rust
#[derive(clap::ValueEnum)]
enum BuildMode { Debug, Release }   // Profile added later
```

`--profile` is parsed and rejected ("profile mode is not supported by the Rust
shell yet") so adding it later doesn't break the CLI. **Every**
mode-dependent decision goes through one `ModeConfig` lookup so profile is a
one-place addition:

| | Debug | Release |
|---|---|---|
| Cargo | default (`dev`) | `--release` |
| Engine variant | `host_debug` / `android_debug_<arch>` | `host_release` / `android_release_<arch>` |
| Dart output | kernel (`copy_flutter_bundle`) | AOT `app.so` |
| Gradle | `assembleDebug` | `assembleRelease` |
| `run` | launch + `flutter attach` (hot reload, DevTools) | launch + log streaming only |
| Plugins | include dev dependencies | exclude dev-only dependencies |

(The runner template's `[profile.flutter-profile]` already exists for later.)

### `rlutter create`

Calls the stock `flutter create` so the Rust shell can **coexist with the
native shells**: a project can have `android/`, `linux/`, `web/`, … from
Flutter *and* `runner-rs/` (+ `android-rs/`) from rlutter.

1. Parse rlutter's own options. `--rust-platforms` (default `linux`) picks
   which Rust shells to generate. Everything after `--` (including
   `--platforms`, `--empty`, `--sample`, `--description`, …) is forwarded
   unchanged. `--org` and `--project-name` are forwarded **and** read, because
   rlutter needs them for the applicationId and crate name.
2. Run `flutter create <forwarded…> <dir>`.
   - Stock create needs at least one platform (`create.dart:505`). If the
     user forwarded no `--platforms`, rlutter passes `--platforms=linux` and
     deletes the generated `linux/` afterwards, so the result has only the Rust
     shell(s). That's isolated in one function (`create::scaffold_without_native_shells`).
   - If the user forwarded `--platforms`, nothing is deleted; native shells coexist.
3. Patch `pubspec.yaml`: add `rust-shell: true` (the manifest validator allows
   unknown top-level keys), keeping comments/formatting (line-based insertion,
   not a YAML round-trip).
4. Write `runner-rs/` and, if requested, `android-rs/` from templates embedded
   with `include_str!` (moved from `flutter_tools/templates/rust_shell`).
   Extend `.gitignore`.
5. Run `flutter pub get` (with local-engine flags) and plugin refresh, so the
   project builds immediately.

`rlutter create` on an existing project (no `<dir>` contents created by
`flutter create`) only adds the Rust shell(s): detect an existing
`pubspec.yaml` and skip step 2.

### `rlutter build linux`

1. `sdk::ensure(Linux, mode)`.
2. `flutter pub get`, then `plugins::refresh(mode)`.
3. Dart side via `flutter assemble --local-engine… -dTargetPlatform=linux-x64 -dBuildMode=<mode> -dTargetFile=<t> --output=build/rlutter/linux/<mode>`:
   - debug: `copy_flutter_bundle`
   - release: `release_flutter_bundle aot_elf_release`, then copy `app.so`
     into `flutter_assets/` (what `RustAotBundle` does today).
4. `cargo build [--release] [--locked]` in `runner-rs/`, then build native libs.
5. Stage `runner-rs/target/<profile>/{data/{flutter_assets,icudtl.dat},libs/*.so}`,
   the same layout `flutter build rust` produces today, so the runner template doesn't change.

### `rlutter build android`

1. `sdk::ensure(Android, mode, abis)`. Fail clearly for a missing ABI: "no
   x86_64 engine artifacts; build `android_debug_x64` or wait for published artifacts".
2. `flutter pub get`, `plugins::refresh(mode)`.
3. Dart side:
   - debug: `copy_flutter_bundle` with `-dTargetPlatform=android-arm64`
     (the kernel is ABI-independent; done once).
   - release, per ABI: `android_aot_bundle_release_android-<arch>` → `app.so`.
4. Per ABI: `cargo build --target <triple> [--release]` for `runner-rs` as a
   `cdylib`, with NDK env (`CC_<triple>`, `CXX_<triple>`, `AR_<triple>`,
   `CARGO_TARGET_<TRIPLE>_LINKER`, the same as `BUILD.gn` around line 269).
   Native plugin libs get the same treatment.
5. Stage `.dart_tool/flutter_rs/android/<mode>/`:
   - `jniLibs/<abi>/{libflutter_rust_engine.so, lib<app>.so, libapp.so (release), <native plugin libs>}`
   - `assets/{flutter_assets/…, icudtl.dat, rlutter_build_id}`
6. `./gradlew assemble<Mode>` in `android-rs/`, which reads `jniLibs` and
   `assets` from the staging dir. Print the APK path.

### `rlutter run`

```
build → launch → wait for VM service URI → (debug) flutter attach → cleanup
```

- **Device selection**: `devices::discover()` lists `linux` (the host) plus
  `adb devices -l` (serial, model, ABI from `getprop ro.product.cpu.abi`).
  Use `-d` if given, else the only device, else prompt. The ABI picks the build target.
- **Linux launch**: spawn the runner with the assets dir as argv[1] (and
  `LD_LIBRARY_PATH` for native libs, as `RustShellDevice` does). Tee stdout/stderr.
- **Android launch**: `adb install -r`, `adb shell am start -n <id>/<activity>`,
  `adb logcat --pid=<pid>` (pid via `pidof`, retried).
- **VM service URI**: scrape `The Dart VM service is listening on <uri>`.
  On Android, `adb forward tcp:0 tcp:<device-port>` and rewrite the URI.
- **Debug**: exec `flutter attach -d <device-id> --debug-url <uri>
  --local-engine… -t <target>` with inherited stdin/stdout, so `r`/`R`/`d`/`q`
  and DevTools work as usual. `--no-devtools` maps to `--no-devtools` on attach.
- **Release**: keep streaming logs until the app exits or Ctrl-C.
- **Cleanup** (on exit, Ctrl-C, or attach exiting): kill the runner, or
  `am force-stop`, and remove the adb forward.

### `rlutter devices` / `rlutter clean`

- `devices`: the discovery table above. Mark ABIs that have no engine artifacts as unsupported.
- `clean`: remove `runner-rs/target`, `build/rlutter`,
  `.dart_tool/flutter_rs/{native_libs,android}`, `android-rs/{build,app/build,.gradle}`,
  then run `flutter clean`. Keep `.dart_tool/flutter_rs/sdk` (expensive to re-download).
  `--all` also removes it.

## Android app work

This is the largest new piece; nothing app-level exists yet.

1. **Runner template**
   - `runner-rs/Cargo.toml`: `crate-type = ["rlib", "cdylib"]` on the lib.
     Android-only dependencies (`android_logger`, `winit` with
     `android-game-activity`) go under `[target.'cfg(target_os = "android")']`.
   - `runner-rs/src/android.rs` (`#[cfg(target_os = "android")]`): `android_main`,
     generated from `flutter-shell-android-runner/src/lib.rs`, calling the
     same `register_application` the Linux `main.rs` uses.
   - Replace the hardcoded files dir with `AndroidApp::internal_data_path()`.
   - Assets: on launch, if `files/rlutter_build_id` differs from the APK's
     `assets/rlutter_build_id`, extract `flutter_assets/` and `icudtl.dat` from the
     APK (through `AndroidApp::asset_manager()`) into the files dir. Release loads
     `<nativeLibraryDir>/libapp.so`.
   - `build.rs`: Android branch with `-rpath,$ORIGIN` (from the android runner's `build.rs`).
2. **Gradle template** `android-rs/`, generated from `android_shell_app`:
   `applicationId`/`namespace` from `--org` + project name, app label from
   pubspec. `jniLibs.srcDirs` / `assets.srcDirs` point at the staging dir. Keep
   the appcompat/kotlin-stdlib fixes, the GameActivity theme, and
   `useLegacyPackaging`. Ship a Gradle wrapper.
3. **Toolchain discovery**: `ANDROID_HOME`/`ANDROID_SDK_ROOT`, else
   `flutter config --machine` (`android-sdk`). NDK from `ANDROID_NDK_HOME` or
   the newest `ndk/*` under the SDK. JDK from `JAVA_HOME`, else
   `flutter config`'s `jdk-dir`. Check that `rustup target list --installed`
   includes the triple; if not, print the `rustup target add` command.
4. **Coexistence**: `android-rs/` has its own applicationId suffix (default
   `<org>.<name>.rs`) so it can be installed side by side with the stock
   `android/` app. Configurable in pubspec under `rust-shell:` later; v1 uses the default.

## ABI design (arm64 now, everything later)

- One table: `Abi { android_name, rust_triple, flutter_target_platform, engine_variant_suffix, ndk_clang_prefix }`
  covering `arm64-v8a`, `x86_64`, `armeabi-v7a`, `riscv64`.
- `SUPPORTED_ABIS` (v1 = `[arm64-v8a]`) gates what's accepted. Everything else
  iterates over the requested ABI list: SDK layout, cargo, AOT, jniLibs.
- Device discovery always reports the real ABI, so running on an x86_64
  emulator fails clearly instead of installing an APK that crashes.
- Adding emulator support then means: build/publish `android_{debug,release}_x64`,
  add `x86_64` to `SUPPORTED_ABIS`, and test.

## Crate layout

`engine/src/flutter/shell/platform/rust/crates/rlutter` (shares the workspace
toolchain and lockfile; host-only, not part of the GN build).

```
src/
  main.rs            # clap entry
  cli.rs             # derive definitions, BuildMode, TargetPlatform, Abi
  mode.rs            # ModeConfig lookup
  project.rs         # pubspec (serde), paths, rust-shell detection
  flutter.rs         # `flutter` wrapper: root discovery, local-engine flags, create/pub get/assemble/attach
  sdk/               # ensure(), local-checkout symlinks, download + unpack, variant layout
  plugins/           # discovery (cargo_metadata), Cargo.toml section, registrant, native libs, runner regeneration
  cargo.rs           # host + NDK cross builds
  android/           # sdk/ndk/jdk discovery, gradle, adb
  device/            # trait Device; linux.rs, android.rs; discovery
  commands/          # create, build, run, devices, clean
  templates/         # runner-rs/**, android-rs/** (include_str!)
```

Dependencies: `clap` (derive), `serde`, `serde_json`, a maintained serde YAML
crate (`serde_norway` or `serde_yml`; `serde_yaml` is archived), `cargo_metadata`,
`anyhow`, `which`, `ctrlc`, `flate2` + `tar` (SDK archive), `ureq` (download).
No async runtime; threads handle log pumping.

Distribution: `cargo install --path …/crates/rlutter` for now, plus a
`Taskfile.yml` task. Later, ship a prebuilt binary in the SDK archive.

## Phases

### Phase 0: spike (gate everything else on this)

Use throwaway scripts, no rlutter code. Prove:

1. `flutter assemble` + `--local-engine=<abs>/src/out/host_debug` against the
   symlinked layout produces a kernel the Rust-shell engine accepts (no SDK hash error).
   Same for `aot_elf_release` and `android_aot_bundle_release_android-arm64`.
2. `flutter attach -d <id> --debug-url <uri> --local-engine…` hot-reloads and
   hot-restarts a running Rust-shell app:
   - **Android**: should work, since the stock `AndroidDevice` matches the serial.
     Check that attach doesn't reject the project for lacking `android/`.
   - **Linux**: stock `LinuxDevice.isSupportedForProject` requires `linux/`.
     Find out whether `attach` filters on that. If it does, options in order:
     (a) an empty stub `linux/` dir, (b) a `flutter-attach`-compatible device id
     trick, (c) our own reload driver (below).
3. DevTools launches from attach on both.

**Fallback if attach doesn't work**: an in-CLI reload driver. Run
`frontend_server` in incremental mode, upload over DevFS or a direct file
copy, call `reloadSources` and `_flutter.runInView` over the VM service, and
start DevTools with `dart devtools --machine`. That's a bigger piece of work;
re-plan if we end up here.

### Phase 1: scaffold, SDK, plugins, create, clean

- Crate, clap tree, `ModeConfig`, `project`, `flutter` wrapper.
- `sdk::ensure` for Linux (local checkout + download), including the new `engine/src/out` layout.
- Port `plugins` (with the `rust_plugins_test.dart` cases as Rust unit tests).
- Templates moved; runner v5 (`lib/<platform>-<mode>`).
- `rlutter create` (Linux shell only), `rlutter clean`.

**Done when**: `rlutter create foo && cd foo && cargo build --manifest-path runner-rs/Cargo.toml` works.

### Phase 2: Linux build

- `rlutter build linux --debug|--release`.

**Done when**: the output tree matches `flutter build rust` today and runs, with
`examples/multiple_windows` and `examples/texture` plus one frb native-lib project.

### Phase 3: Linux run

- Release: launch + logs. Debug: URI scrape + attach.
- `rlutter devices` (host only).

**Done when**: hot reload, hot restart, and DevTools work on Linux; Ctrl-C leaves no orphaned process.

### Phase 4: Android build

- `Abi` table, NDK/SDK/JDK discovery, Android runner + Gradle templates,
  asset extraction, `rlutter create --rust-platforms=android`.
- SDK layout for `android_{debug,release}_arm64`. Decide on the artifact archive shape and add it to CI.

**Done when**: debug and release arm64 APKs install and render on a physical device.

### Phase 5: Android run

- adb discovery with ABI, install, launch, logcat, forward, attach, cleanup.

**Done when**: hot reload, hot restart, and DevTools work on a physical arm64 device; an
x86_64 emulator gives a clear "unsupported ABI" error.

### Phase 6: cut over

- Remove every Rust-shell change from `flutter_tools` (the table in "Current
  state"), including tests. `git diff master -- packages/flutter_tools` should be empty.
- Point `Taskfile.yml` tasks and the docs (`flutter-rs.md`, `flutter-rs-proggress.md`) at rlutter.
- Retire `android_shell_app`, or keep it as an engine-dev-only scaffold.

### Later

- Emulator + other ABIs (artifacts + `SUPPORTED_ABIS`).
- Profile mode (`ModeConfig` row, engine variants, `flutter-profile` cargo profile).
- `AAssetManager` asset loading (drop extraction), `.aab`, release signing.

## Testing

- Unit: arg parsing, `ModeConfig`, `Abi` table, plugin discovery/registrant
  generation (fixtures with fake `cargo metadata` output), pubspec patching,
  VM-service URI scraping, local-engine variant mapping.
- Integration (Taskfile, not CI at first): `create → build → run` for Linux
  debug/release on the examples; Android on a connected arm64 device.

## Risks

| Risk | Mitigation |
|---|---|
| `flutter attach` rejects the project/device on Linux | Phase 0 gates; stub dir or own reload driver |
| Stock tool changes `--local-engine` path parsing or assemble target names | Pin in Phase 0 tests; one module (`flutter.rs`) owns all invocations |
| `flutter create` workaround (generate + delete `linux/`) breaks | Isolated in one function; an upstream "no platforms" option would remove it |
| Asset extraction slows startup | Build-id stamp means extraction only after an install; `AAssetManager` later |
| Plugins with Android native code (Java/Kotlin) | FlutterActivity and the standard Flutter Gradle plugin build and register them; release verification compiles a fixture that uses JNI plugins. |
- 2026-10-19: Phase 2 done (`rlutter build linux [--debug|--release] [-t ...]`, 47 unit tests). Verified on a generated app:
  - debug: runner + `data/flutter_assets` + `libs/libflutter_rust_engine.so`; the built binary started with no
    arguments finds its assets next to itself and boots the Dart VM (VM service banner printed).
  - release: `data/flutter_assets/app.so` is an x86-64 ELF; the runner stays alive with only the benign
    winit warning in its log.
  - plugins: a local Dart package with `rust/Cargo.toml` + `plugin = true` is linked through the generated
    `[BEGIN/END FLUTTER GENERATED RUST PLUGINS]` section and `flutter_plugins.rs`; its registrar ran at startup.
    A `cdylib`-only package was built with its `rustflags`, staged in `libs/` and exports its symbol.
  - Not covered: icon tree shaking (not passed to assemble; release bundles are not tree-shaken yet), and
    the `--locked` flag interacting with a changing plugin set (inherited from the Dart tool's behaviour).
- 2026-10-19: Phase 3 code written (`rlutter run`, `rlutter devices`, `device::{Device, LinuxHost}`).
  Deviations: no `--no-devtools` (`flutter attach` has no such flag; DevTools comes up with DDS by default, so it
  is simply on) and no trailing app args (the runner treats argv[1] as the assets path). Several devices are
  chosen with a prompt only on a terminal, otherwise `-d` is required.
- 2026-10-19: Phase 3 verified with the generated app (`rlutter run`):
  - debug: build -> launch -> VM service scraped from the runner output -> `flutter attach -d linux --debug-url`.
    A real edit hot-reloaded ("Reloaded 1 of 1040 libraries"), hot restart worked, the DevTools URL was printed,
    `q` exited rlutter with code 0 and left no runner behind (attach only detaches; rlutter stops the runner).
  - release: build -> launch -> log streaming. SIGINT (Ctrl-C) stops the runner and rlutter exits.
  - debug with SIGINT and no stdin: the runner is killed, attach reports "Lost connection to device." and exits;
    no process left over.
  - `rlutter devices` lists the host as `linux`.
- 2026-10-19: Phase 4 implemented: `rlutter build android [--debug|--release] [-t ...] [--target-platform android-arm64] [--org ...]`
  and `rlutter create --rust-platforms=linux,android`. New modules: `abi`, `android` (SDK/NDK/JDK discovery),
  `android_runner` (idempotent runner patching), `android_project` (generated `android-rs/` Gradle project, user-owned
  after creation), `commands/build_android`. 79 unit tests.
  - **Design as built.** Runner becomes an Android library with `cargo rustc --lib --crate-type cdylib --target ... `, so
    `Cargo.toml` keeps `rlib` only and Linux builds are unchanged. `rlutter` appends a generated
    `[BEGIN/END FLUTTER GENERATED ANDROID RUNNER]` block (android-only deps) to `runner-rs/Cargo.toml`, adds
    `mod flutter_android;` to `src/lib.rs`, and owns `src/flutter_android.rs` (the `android_main` entry point; rewritten on
    every build). The entry point extracts `flutter_assets/` + `icudtl.dat` from the APK into the app's files dir when
    `rlutter_build_id` changes, using `rlutter_assets.txt` because the NDK asset API cannot list directories. Release
    loads the AOT code as `libapp.so` by name from `jniLibs`.
  - **JDK.** The system `java` is JDK 27, which this Gradle/AGP combination does not run on. `rlutter` mirrors Flutter's
    lookup instead: Android Studio's bundled JBR (JDK 25 here), then `JAVA_HOME`. Gradle 9.0.0 + AGP 8.5.2 are the
    combination the old `android_shell_app` already used; it built in ~10s offline with the JBR.
  - **Verified on this machine (no device):** `rlutter build android` on a project originally created for Linux produces
    `app-debug.apk` (`aapt2 dump badging`: package `com.example.demo_app.rs`, launchable activity `GameActivity`, INTERNET,
    `native-code: arm64-v8a`). `libdemo_app.so` is an aarch64 ELF exporting `android_main`, needing
    `libflutter_rust_engine.so` with `RUNPATH $ORIGIN`, and contains the Rust plugin; `librlnative.so` (native lib) is aarch64.
    Assets in the APK: kernel, snapshots, `icudtl.dat`, `rlutter_assets.txt`, `rlutter_build_id`. The debug APK is 227 MB
    because the unoptimized debug engine (441 MB uncompressed) is not stripped.
  - **Not verified: release.** `rlutter build android --release` fails in `kernel_snapshot_program`:
    `Hyphens isn't a type`, `Member not found: 'slider'`. The cause is the local engine, not rlutter:
    `out/android_release_arm64` was built 2026-09-14, but the framework gained `Hyphens` on 2026-10-02 (3c5450855c1), so its
    `flutter_patched_sdk` (dart:ui) is stale. `host_release` and `android_debug_unopt_arm64` were rebuilt on 10-19.
    **To finish verifying:** rebuild that variant, e.g.
    `ninja -C engine/src/out/android_release_arm64 libflutter_rust_engine.so flutter_patched_sdk` (same environment as the
    Taskfile's `build-rust-shell`), then rerun `rlutter build android --release`. Nothing after the Dart compile has run:
    AOT through `clang_x64/gen_snapshot`, the release cargo cross-build, `libapp.so` staging, `assembleRelease`.
  - Not done: 16 KB page-size alignment flag for the runner (needed for new Play Store apps on Android 15+), stripping,
    `AAssetManager` loading (extraction is the v1 approach), x86_64 emulator (needs `android_{debug,release}_x64` engines).
- 2026-10-19: Phase 5 code written, **not run against a device** (none was attached; `adb devices` was empty):
  `adb` wrapper (device list with `-l`, abi via `ro.product.cpu.abi`, install, `am start`, pid polling, logcat,
  `adb forward` with free host port, force-stop) and `device::android::AndroidDevice` (build -> install -> launch -> logcat
  scrape of the VM service URI -> forward -> rewrite URI -> `flutter attach -d <serial>`). Parsing and command construction are
  unit tested. `rlutter devices` also lists adb devices that are unauthorized/offline. Open questions only a device can answer:
  whether stock `flutter attach -d <serial> --debug-url` accepts our forwarded URI without `--app-id`, and whether
  `logcat --pid` replays the banner printed before it attached.
- 2026-10-19: Phase 6 done. The branch's changes to `packages/flutter_tools` were reversed in the working tree
  (`git apply -R` of `git diff <merge-base> HEAD -- packages/flutter_tools`; 28 files, 2.2k diff lines; nothing committed,
  `git checkout HEAD -- packages/flutter_tools` restores them).
  `git diff master -- packages/flutter_tools` is empty. Re-verified from scratch against the pristine tool:
  `rlutter create` -> `build linux` -> `run` hot-reloaded a real edit ("Reloaded 1 of 1040 libraries"), hot-restarted, served
  DevTools, exited 0 with no leftover runner; the "Regenerated ... runner version 4" interference is gone. Then
  `build android` (debug) on that Linux-created project auto-generated `android-rs/` and produced the APK.
  `Taskfile.yml`: `build-rlutter`, `install-rlutter`, `test-rlutter`; `fmt-rust` also formats rlutter. `flutter-rs.md`
  tooling section now describes `rlutter`. Clippy `-D warnings` clean, 79 unit tests.
  Caught and fixed on the way: `create` regressed to not linking the host engine when Android support was added
  (a fresh project then failed `cargo build`); now fixed and checked with `cargo check --locked` on a fresh project.

- 2026-10-19: Device selection reworked. `-d` accepts an exact id (so `-d linux` is still the host) or a platform name
  (`android`, `linux`) that filters candidates; a single match is taken without asking, several open an arrow-key picker
  (`inquire`) when stdin and stderr are terminals, otherwise the error lists the ids. Esc cancels. The decision logic is a pure
  function (`device::choose`) with unit tests; the picker was driven through a pty with a stub `adb` in a fake SDK
  (Esc, Down+Enter, `-d android`, unknown id). 82 unit tests, clippy clean.

- 2026-10-19: Window title is now delegated to the app. The shell (`flutter-shell-winit`) handles
  `SystemChrome.setApplicationSwitcherDescription` (what `MaterialApp.title` sends) and applies the label to the main
  window; empty labels are ignored so the runner's title stands. The Linux runner starts with the project name instead of the
  default "Flutter Rust Shell". Runner version bumped to 6 (existing projects regenerate `build.rs`/`main.rs`/
  `flutter_android.rs`). Unit tests for the decoder (36 shell tests, 82 rlutter tests, clippy/fmt clean). Confirmed working by
  the user on their desktop; I did not run my own check against the compositor (the attempt was declined).
  Not done: titles of additional windows (the multi-window API already sets those natively) and Android (no window title).

### Still open

1. **Android release** - rebuild `out/android_release_arm64` (stale dart:ui), then run `rlutter build android --release`.
2. **Everything on a device** - `rlutter run -d <serial>` (install, launch, logcat, forward, `flutter attach`), debug and
   release; also that the extracted-assets startup path really works on a phone. Nothing in Phase 5 has run.
3. BETA SDK download path in `sdk::ensure_sources` (only the local-checkout path was exercised).
4. x86_64 emulator, other ABIs, profile mode, 16 KB page alignment, icon tree shaking, Windows/web/macOS/iOS.
