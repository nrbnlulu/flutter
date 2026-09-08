---
name: fvm
description: Install, select, and update the nrbnlulu Flutter Rust-shell fork with FVM 4 or newer. Use when configuring a project for the rolling BETA fork or ensuring Flutter commands use the correct SDK.
---

# Use the Rust-shell fork with FVM

FVM 4 or newer can manage named Flutter forks. Check the installed version and
configured aliases:

```sh
fvm --version
fvm fork list
```

If the `nrbnlulu` alias is missing, add it:

```sh
fvm fork add nrbnlulu https://github.com/nrbnlulu/flutter.git
```

Install the rolling BETA and select it in the application directory:

```sh
fvm install nrbnlulu/BETA
fvm use nrbnlulu/BETA
```

Commit `.fvmrc` so the project records its SDK selection. Do not commit the
machine-local `.fvm/` directory. Verify the active SDK:

```sh
fvm flutter --version
fvm doctor
```

Use `fvm flutter ...` and `fvm dart ...` for SDK commands. A plain `flutter`
command may select an upstream SDK that does not support the Rust shell.

## Create an application

Before a project and `.fvmrc` exist, run the create command through the named
SDK:

```sh
fvm spawn nrbnlulu/BETA create --shell=rust --platforms=linux my_app
cd my_app
fvm use nrbnlulu/BETA
```

Then resolve the application:

```sh
fvm flutter pub get
```

The fork downloads its Rust-shell SDK into `.dart_tool/flutter_rs/sdk`. The
current BETA package supports Linux x64.

## Update the rolling BETA

The `BETA` tag moves when a new fork build is published. FVM reuses an existing
cached installation, so reinstall the selector when the user requests the
latest BETA:

```sh
fvm flutter --version
fvm remove nrbnlulu/BETA
fvm install nrbnlulu/BETA
fvm use nrbnlulu/BETA --skip-pub-get
fvm flutter pub get
```

Removing this SDK affects every local project using the same cached selector;
do not refresh it while another build is running. If the project still has an
older generated Rust-shell SDK, remove `.dart_tool/flutter_rs/sdk` and rerun
`fvm flutter pub get`.

Use an immutable fork commit or release tag instead of `BETA` when a build must
remain reproducible. Keep the `nrbnlulu/` prefix in the FVM selector.
