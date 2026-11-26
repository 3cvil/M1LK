<p align="center"><img src="docs/assets/m1lk-logo.svg" width="160" alt="M1LK logo"></p>

# M1LK

M1LK is a low-end NVIDIA-focused Xbox Series compatibility runtime for Windows x64, based on
[VIctory](https://github.com/Marvisak/VIctory) and the [DexNom fork](https://github.com/DexNom/VIctory).
It loads an Xbox Series title's x86-64 executable from an installation you supply, provides the
console platform services underneath it, and translates its GPU work to Direct3D 12.

The target machine for this fork is a PC with an NVIDIA GPU with **8 GB of VRAM** and **16 GB of system
RAM**. AMD Radeon remains supported: the AMD code paths inherited from upstream (including the FSR
upscaler profiles and the experimental ray-tracing path) are kept and are not removed or regressed by
this fork.

The project is experimental. Expect low frame rates, rendering errors and crashes.

## Legal notice

- **This repository contains no game files, assets, keys or executables**, and none are distributed
  with it. Nothing derived from a title (shader dumps, extracted tables, captured textures or
  screenshots) is stored here either. See the [asset policy](docs/asset-policy.md).
- To use M1LK you must supply your **own legally obtained copy** of a title. You are responsible for
  complying with the license terms of any software you run with it.
- M1LK is **not affiliated with, endorsed by or sponsored by Microsoft, Xbox, or any game developer or
  publisher**. "Xbox", "Xbox Series X" and "Xbox Series S" are trademarks of Microsoft Corporation and
  are used only to describe compatibility.

## Requirements

- Windows 10 or 11, 64-bit, and a CPU with AVX2.
- A Direct3D 12 GPU with Shader Model 6.6 and mesh shader support and a current driver. On NVIDIA this
  means a GeForce RTX 20 series card or newer. On AMD, a Radeon RX 6000 series card or newer.
- Several GiB of free Windows commit charge at startup. With 16 GB of RAM, close other programs and
  keep the Windows page file on automatic. If `crashes.log` reports Windows error 1455, increase the page
  file size and retry.
- To build: [rustup](https://rustup.rs) and Visual Studio Build Tools (see below).

## Building from source

1. **Install Rust with rustup.** The toolchain version is pinned in [`rust-toolchain.toml`](rust-toolchain.toml)
   (currently 1.98.1, target `x86_64-pc-windows-msvc`, with `rustfmt` and `clippy`). rustup reads that file
   and installs the pinned toolchain the first time you run `cargo` in the checkout. To install it ahead of
   time, run `rustup toolchain install` in the repository root.
2. **Install Visual Studio Build Tools** (or Visual Studio). In the installer, select the
   **Desktop development with C++** workload and make sure these components are included:
   - MSVC v143 (or newer) x64/x86 build tools
   - a Windows 10 or Windows 11 SDK

   [Microsoft C++ Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
3. **Supply the BCPack format tables.** The texture decoder needs a 77,982-byte table file at
   `crates/victory-formats/src/bcpack-tables.bin`. It is not part of this repository (see
   [Separately supplied data](#separately-supplied-data)). Once you have it, verify it with:

   ```powershell
   .\Initialize-FormatTables.ps1
   ```

   Without arguments the script fetches the file from the original upstream repository at a pinned
   commit. With `-FromCheckout <path>` it copies it from another checkout instead. In both cases it checks
   the SHA-256 hash before installing it. The file is ignored by git.
4. **Build the release binaries:**

   ```powershell
   cargo build --release -p victory --bins
   ```

   The executables are written to `target\release\`:

   | File | Purpose |
   | --- | --- |
   | `m1lk.exe` | The runtime and command-line tool (`run`, `warm`, `inspect`, `translate`, ...) |
   | `m1lk-play.exe` | The launcher: sets up logs, saves and caches next to itself and starts `m1lk.exe run` |
   | `m1lk-warm.exe` | The shader preloader: starts `m1lk.exe warm` with the launcher's settings |

   The Cargo package and crates keep their upstream names (`victory`, `victory-*`), which is why the
   build command uses `-p victory`. On a machine with 16 GB of RAM, limit parallel compilation if the
   build runs out of memory: `$env:CARGO_BUILD_JOBS = '4'`.
5. **Optional: the diagnostics build.** The `diagnostics` feature adds debug-level logging and the
   debugging switches listed under [Debugging](#debugging) (frame capture, stack dumps, the D3D12 debug
   layer, scripted keys). A plain release build leaves them out.

   ```powershell
   cargo build --release -p victory --features diagnostics --bins
   ```

   There is also a `diag` profile, which writes to `target\diag\`:
   `cargo build --profile diag -p victory --features diagnostics`.
6. **Optional: run the tests.** `cargo test --workspace` needs no title data. Some GPU tests need a
   Direct3D 12 adapter.

### One-step setup

`Setup.cmd` (which runs `Setup-M1LK.ps1`) does steps 3 and 4 with the diagnostics feature, asks for your
title's executable, copies the three programs to `%LOCALAPPDATA%\M1LK`, and creates Start-menu
shortcuts named M1LK. To choose the executable from PowerShell instead of the file picker:

```powershell
.\Setup.cmd -GamePath "D:\Path\To\Your\Title\title.exe"
```

## Running a title

Copy `m1lk.exe`, `m1lk-play.exe` and `m1lk-warm.exe` together into a folder **outside** the source
checkout (the runtime refuses to write saves and caches inside the repository). Then either pass the path
to your title's executable:

```powershell
& "C:\M1LK\m1lk-play.exe" --no-rt "D:\Path\To\Your\Title\title.exe"
```

or put that path in a file named `game-path.txt` next to `m1lk-play.exe` and start `m1lk-play.exe`
without a path. Keep the executable in its original installation folder together with its accompanying
files. The launcher keeps its logs, saves and cache next to itself; see
[Where M1LK writes its files](#where-m1lk-writes-its-files).

You can also run the runtime directly:

```powershell
target\release\m1lk.exe run "D:\Path\To\Your\Title\title.exe" --execute --console series-x
```

Direct runs keep their data in `%LOCALAPPDATA%\M1LK\data` and their cache in `%LOCALAPPDATA%\M1LK\cache`.
Both can be changed in `m1lk.toml` (copy [`m1lk.example.toml`](m1lk.example.toml)) or with
`VICTORY_DATA_DIR` and `VICTORY_CACHE_DIR`. Environment variables keep their upstream `VICTORY_` prefix for
now.

If you see a missing `VCRUNTIME140.dll` error, install the
[Microsoft Visual C++ Redistributable](https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist) (x64).

### Warming the shader cache

Without a warm cache, each shader is translated the first time the title uses it, and what it draws is
missing until then. The warm step translates every shader it can find ahead of time:

- Run `m1lk-warm.exe` from the launcher folder. It reads `game-path.txt`, scans the whole title
  installation for supported shader records, translates them into the `cache` folder and creates the
  compute pipelines on your GPU. It uses the same cache and adapter settings as `m1lk-play.exe` and
  writes its own log, named in `logs\latest-warm.txt`. Every 10 seconds it logs its progress, measured
  against the total of the previous warm-up when there is one. The last line (the warm summary) reports
  how many shaders and compute pipelines were made and refused.
- Or run it directly: `m1lk.exe warm "D:\Path\To\Your\Title"` (add `--jobs N` to limit worker threads,
  `--no-pipelines` to translate only).

Graphics pipelines depend on draw state that only appears during play, so they are still created as you
play. The cache key includes the translator, so after updating M1LK run the warm step again. Translations
are made on your machine from your own copy and are never shared.

## Low-end NVIDIA settings (8 GB VRAM / 16 GB RAM)

NVIDIA gameplay has had little testing. These are the suggested starting settings for the target
machine:

```powershell
$env:VICTORY_FSR_4K = '0'
$env:VICTORY_AMBIENT_FALLBACK = '1'
& "C:\M1LK\m1lk-play.exe" --no-rt --console series-x --sparse-textures
```

- `--no-rt` skips the experimental ray-tracing builder, traversal and lighting resolve, which are the
  most expensive passes. Supported material shaders use a lightweight ambient fill instead
  (`VICTORY_AMBIENT_FALLBACK=1`); some interiors may look darker.
- Leave out the FSR presets (`--fsr-quality`, `--fsr-balanced`, `--fsr-performance`). They need AMD's
  FidelityFX provider and render to a 4K output.
- `--sparse-textures` uploads texture mip levels on demand, which lowers peak VRAM use.
- `--console series-s` makes the title choose its Series S settings (lower resolution and memory budgets).
  This can help on 8 GB cards and 16 GB machines but is less tested than Series X.
- `--rt-static-bvh` is a partial ray-tracing experiment: it traces rays against the prebuilt static
  world structure without rebuilding dynamic geometry. It is still expensive.
- `VICTORY_PIPELINE_WAIT_MS=24` shortens the per-frame wait for background pipeline creation (default
  500 ms); draws may be skipped until their pipeline is ready.

## AMD settings

AMD Radeon remains supported. The upstream Radeon profile with ray tracing and FSR Quality at 4K is:

```powershell
& "C:\M1LK\m1lk-play.exe" --rt --console series-x --local-gpu-memory --fsr-quality --sparse-textures --no-upload-ahead --capture-readbacks --rt-write-stats
```

The FSR profiles need AMD's FidelityFX upscaler provider, which is supplied separately: put it in a
`fidelityfx` subfolder next to the launcher or set `VICTORY_FIDELITYFX_DIR` to the folder that contains
`amd_fidelityfx_upscaler_dx12.dll`. See [docs/fork-ray-tracing.md](docs/fork-ray-tracing.md) for the
ray-tracing implementation.

## Launcher options

```
m1lk-play.exe [--rt | --no-rt | --rt-static-bvh] [--warp] [--console series-x|series-s]
              [--local-gpu-memory] [--native-1440p | --1080p-to-1440p | --fsr-quality |
              --fsr-balanced | --fsr-performance] [--sparse-textures]
              [--upload-ahead | --no-upload-ahead] [--capture-readbacks] [--rt-write-stats]
              [--rt-store-trace] [--lighting-trace] [title executable]
```

`m1lk.exe --help` lists the commands of the runtime itself (`inspect`, `census`, `shader-info`,
`translate`, `warm`, `run`, `config`).

## Input

XInput controllers and DualSense are supported. With none connected, the keyboard and mouse drive a
stand-in controller while the title's window has focus: W A S D and the mouse (or I J K L) are the sticks,
the right and left mouse buttons (or Z / C) the triggers, Enter / Backspace / X / Y the face buttons,
Q / E the bumpers, the arrow keys the D-pad, and Esc / Tab are Menu / View. Mouse look, sensitivity and
key rebinding (`VICTORY_KEYBINDS`) are described in [docs/keyboard-and-mouse.md](docs/keyboard-and-mouse.md).

## Where M1LK writes its files

| What | Launcher (`m1lk-play.exe`, or a Setup.cmd install) | Direct `m1lk.exe run` |
| --- | --- | --- |
| Install folder | `%LOCALAPPDATA%\M1LK` (or wherever you copied the programs) | - |
| Run log | `logs\run-<time>-<id>.log`; `logs\latest.txt` names the latest | the console (standard error) |
| Shader warm log | `logs\warm-<time>-<id>.log`; `logs\latest-warm.txt` names the latest | the console (standard output) |
| Crash log | `state\<title id>\crashes.log` | `%LOCALAPPDATA%\M1LK\data\<title id>\crashes.log` |
| Saves | `state\<title id>\storage` | `%LOCALAPPDATA%\M1LK\data\<title id>\storage` |
| Shader and pipeline cache | `cache` | `%LOCALAPPDATA%\M1LK\cache` |

The cache, saves and any captures are derived from your copy of the title. Never upload them.

## Debugging

These switches need a build with the `diagnostics` feature.

| Variable | Effect |
| --- | --- |
| `VICTORY_LOG=<target>=debug,...` | debug and trace output |
| `VICTORY_STACKS=20,60` | every thread's stack at those seconds |
| `VICTORY_KEYS=30:Enter,35:X` | presses keys at those seconds |
| `VICTORY_CAPTURE=<dir>` | saves presented frames (to a folder outside the repository) |
| `VICTORY_D3D12_DEBUG=1` | the D3D12 debug layer |
| `VICTORY_D3D12_DRED=1` | on a GPU failure, the allocation hit and the commands in flight |
| `VICTORY_D3D12_WARP=1` | runs on the software adapter (slow) |

Captures and dumps are derived from your copy of a title. Keep them on your machine and never commit or
publish them.

## How to contribute

### What we need help with most

In priority order:

1. **RAM use.** In Series X mode the runtime commits about 17 GB and pages heavily on 16 GB machines. We
   need it under about 13 GB.
2. **The safety watchdog crash during the title's first cutscene on low-RAM machines.**
3. **Frame rate on 8 GB NVIDIA cards**, currently single digits to the low teens.
4. **NVIDIA shader translation issues**: programs written for 64-lane waves running on 32-lane hardware
   (wave32/wave64), and lane masks used as plain data.
5. **Ray-tracing correctness on NVIDIA.**

### Reporting a problem

Open an issue with the **Bug report** form. It asks for:

- **PC specs (required):** GPU model and VRAM, GPU driver version, CPU, amount of RAM, Windows version
  and build, and the storage type the title is installed on (SSD or HDD).
- The M1LK commit you built, the console mode (`series-x` or `series-s`), and the launcher flags and
  `VICTORY_*` environment variables you used.
- What happened, how far the title got, the frame rate, and the peak memory use (Task Manager's
  **Committed** figure and dedicated GPU memory).
- **Logs.** For build problems, attach the full `cargo` output. For runtime problems, attach the run log,
  every `crashes.log` and the shader warm summary (the last line of the warm log). Their locations are
  listed in [Where M1LK writes its files](#where-m1lk-writes-its-files). Logs are required for crashes.

The easiest way to gather all of this is the log collector. It zips the latest run and warm logs, every
`crashes.log`, an optional build log and a specs summary, after replacing your user name and profile path
with `<user>` and your computer name with `<pc>` and removing IP and e-mail addresses:

```powershell
powershell -ExecutionPolicy Bypass -File tools\collect-logs.ps1
# for a build problem, save the cargo output first and pass it in:
cargo build --release -p victory --bins *> build.txt
powershell -ExecutionPolicy Bypass -File tools\collect-logs.ps1 -BuildLog build.txt
```

Attach the zip it writes to the issue. Open it and check it before you upload it.

**Do not attach** title files, saves, shader caches, frame captures, memory dumps, or anything else
containing data from a title, and do not post personal information (user names in paths, IP addresses,
e-mail addresses).

### Sending a pull request

- Keep pull requests small and focused on one problem.
- Run `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`
  before you open it, and make sure they pass.
- Say which hardware you tested on (GPU, VRAM, driver, CPU, RAM) and give before-and-after numbers for
  the same scene and settings: frame times, committed memory, VRAM.
- Keep AMD paths working. Changes for NVIDIA must not remove or regress Radeon support.
- If you port a change from another fork, credit the source in the pull request and the commit message.
  Its license must be compatible with GPL-3.0-or-later.
- No game names, title files or anything derived from them in code, comments, tests or commits (see the
  [asset policy](docs/asset-policy.md)). Describe titles neutrally, e.g. "the title".

The pull request template has a section for the hardware and the before-and-after numbers.

## Separately supplied data

- **BCPack format tables** (`crates/victory-formats/src/bcpack-tables.bin`). The texture decoder needs
  these constant tables at build time (`include_bytes!`). They are not licensed under the GPL with the
  rest of the source, so this repository does not contain them. Supply the file yourself and verify it
  with `Initialize-FormatTables.ps1` (expected SHA-256
  `1F1D4760419202BB5672F0C428CE51E456B5D08FBAEC8F371D043FD94E01C36B`).
- **Your title.** The executable, its DLLs and data stay in your own installation folder and are read in
  place.
- **AMD FidelityFX upscaler** (optional, for the FSR profiles).

## Credits

M1LK is a fork. It is based on:

- **VIctory** by **Marvisak** and the VIctory contributors, who wrote the original runtime: the loader,
  the platform services, the Direct3D 12 back end and the shader recompiler.
  [Marvisak/VIctory](https://github.com/Marvisak/VIctory)
- The **DexNom** fork of VIctory and its contributors. [DexNom/VIctory](https://github.com/DexNom/VIctory)

Upstream also credits:

- **ZSlayerHQ and the victory-world contributors**, including triippywhale and asdaasdsaddf, for Radeon
  rendering work. [victory-world-runtime](https://github.com/ZSlayerHQ/victory-world-runtime)
- **The XCL contributors**, whose independent Xbox Series compatibility layer informed upstream work on
  shader translation and memory synchronization. [XCL](https://github.com/xcl-dev/xcl)
- **AMD** for the FidelityFX/FSR upscaler used by the optional FSR profiles.
  [AMD FSR SDK](https://gpuopen.com/amd-fsr-sdk/)

The full upstream commit history and author list are available in the upstream repositories linked above.

## License

GPL-3.0-or-later, the same as upstream VIctory. See [LICENSE](LICENSE). The separately supplied data
listed above is not covered by this license and is not distributed here.
