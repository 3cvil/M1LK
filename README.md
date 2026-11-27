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

In short, M1LK is an Xbox Series emulator-style compatibility layer for Windows. It implements the
GDK / XGameRuntime platform services and the `d3d12_xs` graphics API on top of desktop Direct3D 12.

## Progress

All numbers come from the reference machine: an RTX 4060 Laptop GPU with 8 GB, an i5-12500H CPU and
16 GB of RAM, on Windows 11. Runs now use the Series S profile (smaller title memory budget). The test log below is filled in automatically after every test run.

| Goal | Status |
| --- | --- |
| 1. Boot to the first loaded scene | Done. Menus, the intro and the first cutscene load and draw. |
| 2. Peak memory 13 GB or less | **In progress.** Series S peaks at **16.4-16.9 GB** (Series X 21.3 GB). On a 16 GB laptop free RAM drops to almost nothing, which costs fps. Next: keeping title memory in place instead of copying it (`VICTORY_INPLACE`) and fixing the constants port, which turned out to place nothing yet. |
| 3. Stable for 30 minutes | **Close.** Every recent run lasted its full 15-20 minute test with no crash. A 40-minute soak is next. |
| 4. Full ray tracing on NVIDIA | BVH builder and wave32 fixes are merged. It will be measured once in-game fps is about 5 or more (shadows, then reflections, then full GI). |
| 5. 30 fps with ray tracing | Not yet. In game, without RT: **about 4-5 fps** (5.1 fps median while walking). The graphics card is busy only about 7% of each frame; the rest is CPU-side copying of title memory and waiting on submissions, so that is what we are cutting next. Frame generation numbers will always be reported next to real fps. |

### Test log

| # | Date | Build | Settings | Result |
| --- | --- | --- | --- | --- |
| 1 | 2026-11-26 | memory fix 1 | Series X, `--no-rt`, `VICTORY_TEXTURE_MIP_DROP=1` | Peak 17.2 GB, 5.9 GB of VRAM. Watchdog after 5 min 46 s with heavy paging. |
| 2 | 2026-11-26 | RAM branch | Series X, `--no-rt`, window budget 1 GB, RAM trim 3 GB | Got further (10 min 19 s) but peaked at 20.0 GB, with 7.3 GB of VRAM. Duplicated texture copies (about 5 GB) and a window budget that did not hold were found. |
| 3 | 2026-11-26 | RAM branch + pressure sweeps | Series X, `--no-rt`, mip drop 2 (textures of 512 px and up), window budget 1 GB, RAM trim 3 GB | **20 min with no crash** (the full test). Menus at 30 fps; in game a median of 0.5 fps, with frames spending 0.9 s (median) to 9.6 s (worst quarter) waiting on paging. Peak 21.6 GB, 6.8 GB of VRAM. The largest memory buckets were made shared buffers (8.1 GB), title window copies (5.2 GB) and host textures (5.0 GB). |
| 4 | 2026-11-26 | merged build (all of today's work) | Series X, `--no-rt`, mip drop 2, window budget 1 GB, windows in Resizable BAR card memory, constants on the card, 2 MB sections; scripted keyboard and mouse play | **20 min with no crash, 5.6x faster:** median **2.8 fps** (was 0.5), worst quarter 1.1 fps (was 0.08). Paging stalls went from 80% of the median frame to none. Walking, sprinting, strafing, jumping, mouse look and the pause menu all respond. The top costs now are copy-back (170 ms) and buffer copies (100 ms). Peak commit 20.0 GB, working set 5.9 GB, 7.7 GB of VRAM. |
| 5 | 2026-11-26 | + skip write-combined copy-back, + window tails | as above, plus `VICTORY_SKIP_WC_BACK=1` and `VICTORY_WINDOW_TAILS=1` | 20 min, no crash. **Median 3.6 fps** (was 2.8): copy-back fell from 170 ms to 28 ms a frame. The worst quarter got worse (0.7 fps, was 1.1) because window tails grew window memory (4.6 GB, was 3.7) and paging came back. Tails are dropped; copy-back skip is kept. |
| 6 | 2026-11-26 | new build: hair skip, UI mips kept, quiet-copy skip, sliced heap rescan, texture read-backs on a thread, wave32 groupshared guard | as above (copy-back skip on) | 20 min, no crash. **Worst quarter 3.0 fps** (was 0.7), so the bad frames are almost gone; median 3.3 fps (3.6 before, in a different, busier scene). No paging stalls at all. About 3,400 unchanged copies a frame are now skipped, and copies fell to 71 ms. Hair renders cleanly (strands are left out for now). |
| 7 | 2026-11-26 | + wide refresh | as above, plus `VICTORY_WIDE_REFRESH=1`, 15 min with play | Median 3.5 fps (3.3 without), within noise, so it stays optional. The ~1,400 draws a frame shown as dropped are all ray-tracing stages (off in `--no-rt`) and left-out hair strands, so nothing is missing by mistake. |
| 8 | 2026-11-26 | Series S fix | Series S, `--no-rt`, 8 min | **No device removal** (it used to happen at about 2.5 min). The shared-exponent target uses its stand-in format, and the run reached 7 min 48 s at 5.4 fps while the scene was still loading. |
| 9 | 2026-11-26 | Series S fix | Series S, `--no-rt`, 15 min with play | Stable, **median 5.3 fps, peak 17.3 GB**, but the picture was black in game and the menus were missing. |
| 10 | 2026-11-26 | Series S picture fix | Series S, `--no-rt`, 15 min with play | **Picture and pause menu show.** Median 4.5 fps, peak 17.1 GB, no crash. The scene is as dark as on Series X (lighting without ray tracing). |
| 11 | 2026-11-27 | + ambient light stand-in, title's own upscaler | Series S, `--no-rt`, `VICTORY_UPSCALER=6`, 15 min with play | Median 4.6 fps, peak 16.4 GB, no crash. Rooms are still very dark. Found later: floating white blocks and orange speckles in one outdoor scene (Series S only; being fixed). |
| 12 | 2026-11-27 | same build, repeat | as above | Standing 3.9 fps, **walking 5.1 fps** (low 4.3), peak 16.9 GB, no crash. Same white blocks and speckles. Free RAM fell to 45 MB during the run. |
| 13 | 2026-11-27 | DLSS fix, copy changes, fork fixes | Series S | Not valid: these test builds were made without the scripted-input feature, so they waited at a menu. Being re-run with the right build. |
| 14 | 2026-11-27 | fork fixes (shader NaN, helper lanes, cache, compute waits) + DLSS fix | series-s, --no-rt | standing/all median 4.5 fps (91 samples), peak 5942 draws/frame; WALKING median 5.4 fps, low 3.6 (32 samples). Picture: clean in the window scene, no white blocks; small pale smear at the left edge. |
| 15 | 2026-11-27 | fork fixes, shader loop limit 1024 (was 32) | series-s, --no-rt, VICTORY_SHADER_LOOP_LIMIT=1024 | standing/all median 3.3 fps (57 samples), peak 7146 draws/frame; no walking samples; free RAM fell to 0.1 GB and the run stopped at about 10 min. Picture: white specks and a blocky sky in the window view (not seen with limit 32). Result: worse, keep 32. |
| 16 | 2026-11-27 | fork fixes, title memory kept in place (VICTORY_INPLACE=1) | series-s, --no-rt, VICTORY_INPLACE=1 | standing/all median 3.5 fps (126 samples), peak 10636 draws/frame; WALKING median 2.6 fps, low 0.2 (42 samples). Picture: clean, no blocks or specks (flagged shots were ceiling lights); the character's head now draws; room very dark. Not comparable to #14: this run reached a heavier scene (10.6k vs 5.9k draws/frame). Same-session repeat queued. |
| 17 | 2026-11-27 | parallel texture checks (VICTORY_PARALLEL_REFRESH=1) | series-s, --no-rt, VICTORY_PARALLEL_REFRESH=1 | No result: stopped by hand. The test install had lost its saved settings, so the run sat on a first-run menu screen and never reached the world. Rerun queued with the settings restored. |
| 18 | 2026-11-27 | parallel texture checks (VICTORY_PARALLEL_REFRESH=1), rerun | series-s, --no-rt, VICTORY_PARALLEL_REFRESH=1 | Inconclusive: the option did not take effect in this build (it covers only one of the texture paths the title uses), and the run started with less free memory and reached a smaller scene, so its frame rates are not comparable. Prologue/standing median 0.8 fps (35 samples), walking median 1.6 fps, low 0.1 (30 samples), peak 6729 draws/frame, peak RAM 16.6 GB. In the world, buffer copies took a median 172 ms a frame against 18 ms for texture checks. Picture: mostly intro and cutscene footage; shaded surfaces and backlit people come out near black (the ray-tracing-off fill light is too weak), bloom blows out in one backlit shot. |
| 19 | 2026-11-27 | fill light 0.6 (VICTORY_AMBIENT=0.6; default 0.3) | series-s, --no-rt, VICTORY_AMBIENT=0.6 | Prologue/standing median 4.1 fps (81 samples), walking median 3.8 fps, low 0.9 (43 samples), peak 12789 draws/frame, peak RAM 16.9 GB. The fill light is a picture setting: fps and memory are within run-to-run noise and the runs reached different scenes, so no performance claim is made. Picture: slightly brighter (one dark bunk room clearly better), no washed-out shadows or glaze; still far too dark overall, with backlit people as silhouettes and late interiors near black, so the fill level is not the main cause. |
| 20 | 2026-11-27 | fill light follows the sky (VICTORY_AMBIENT=sky): picture brightness check | series-s, --no-rt, VICTORY_AMBIENT=sky | Prologue/standing median 4.0 fps (89 samples), walking median 3.9 fps, low 2.8 (46 samples), peak 9518 draws/frame, peak RAM 16.5 GB. The fill light is a picture setting: fps and memory are within run-to-run noise (all runs RAM-starved with heavy paging, different scenes), so no performance claim is made. Picture: darker than both the constant 0.3 and 0.6 fill (cell/bunk mean brightness about 40 vs 47 and 54), no new artifacts; the flagged shots were ceiling lights and grass grain. Not adopted. A likely bigger cause of the darkness (a diagnostics loop cap of 32 used in all recent tests) is being tested next. |
| 21 | 2026-11-27 | shader loop cap 1024 instead of 32 (dark picture fix test) | series-s, --no-rt, VICTORY_SHADER_LOOP_LIMIT=1024 | Prologue/standing median 4.4 fps (70 samples), walking median 3.8 fps, low 0.7 (44 samples), peak 7725 draws/frame, peak RAM 15.8 GB. No measurable fps or RAM difference from the 32 cap in this single noisy test (different scene and free RAM; a build ran during the last minute, in the end-of-run fade); frame time is about 250 ms, of which the GPU is only 14-18 ms and CPU-side preparing about 140 ms. Picture: no brighter than at 32 (rooms still dark, characters still silhouettes), no sparks; flagged shots were ceiling lights. This conflicts with an earlier 1024 run that showed a lit character in one scene, so the loop cap is unlikely to be the main cause but is not ruled out; a back-to-back repeat is parked. Next for lighting: a basic ray-tracing run once fps is up. |

This table is updated after every test run.

## Changelog

Newest first. Each bullet is a step that changed what M1LK does or what it measured.

- **2026-11-27:** **Lighting stand-in and the title's own upscaler (opt-in).** Without ray tracing the title's screen-probe lighting comes out empty, so scenes were near black. A new ambient pass fills it with a soft constant light and halves reflections (`VICTORY_AMBIENT=<level>|sky|off`; ported from another VIctory fork). Console upscaler shaders that carry another compiler's code (`ACCC` containers) can now be translated, including both wave64 halves (ported from two other forks); `VICTORY_UPSCALER=6` uses them instead of the spatial stand-in, `VICTORY_ACCC=0` turns the translation off. `VICTORY_UPSCALER=dlss` on Series S no longer asks for the Series X-only 4K preset. DLSS itself still crashes in NVIDIA's NGX core at start-up on our test machine; a fix is being worked on, so leave it off for now.
- **2026-11-26:** **Series S shows a picture.** Two causes. First, on Series S the title renders at 1280x720 and runs its own console upscaler, whose shaders are console machine code that M1LK can't translate yet, so the upscaled image was never written. Series S now defaults to M1LK's spatial upscaler (`VICTORY_UPSCALER` overrides it). Second, the Series S present has two planes, the scene and the interface. M1LK now composites both, reading the plane records at their 72-byte stride (`VICTORY_PLANES=0` turns this off). Both are ported from other VIctory forks (cool_fox, cgbj, mandy-port). Result: **17.1 GB peak, 4.5 fps median**, against 21.3 GB and 3.5 fps on Series X.
- **2026-11-26:** **Series S no longer crashes.** The title asks for a full-screen render target in a packed HDR format (R9G9B9E5) that NVIDIA can't draw into. M1LK now uses a same-size format that NVIDIA supports (R11G11B10), but only when the card reports the original as unsupported, so AMD is unchanged (`VICTORY_RGB9E5_HOST=0` restores the old behaviour). A render-target format that can't be drawn into now drops its draws instead of removing the device. Also: rectangle clears are sent in batches of 16 (NVIDIA's driver can abort at 64; from VItoria-Shyro / Belobug), and the test runner copies the full run log.
- **2026-11-26:** new build. Hair strands are left out (no more black smudges), and menu/loading art keeps full resolution. Unchanged memory copies are skipped (about 3,400 a frame). The heap rescan is spread over 16 frames, and texture read-backs run on their own thread. Groupshared memory is guarded for 32-lane NVIDIA waves. **The worst quarter of frames went from 0.7 to 3.0 fps.** The test runner gained short (8 min), quick (15 min), full and soak (40 min) kinds.
- **2026-11-26:** skipping write-combined copy-back (`VICTORY_SKIP_WC_BACK=1`, from VItoria-Spark) takes the median to **3.6 fps** (was 2.8). Window tails were tried and rejected: they cost RAM. The playtest driver also gained a save library: each run keeps new autosaves, and `run-m1lk.ps1 -FromSave <name>` starts from one.
- **2026-11-26:** the merged build doubles as a real playtest. `tools/testing/play.ps1` drives real keyboard and mouse input: walk, sprint, strafe, jump, a 360° look, and the pause menu. It logs each step and screenshots the menus. The first 20-minute run: **2.8 fps median (was 0.5), 1.1 fps worst quarter (was 0.08)**, with no crash. All the controls work.
- **2026-11-26:** merged all of today's work into one build:
  - RAM savers: windows in Resizable BAR card memory, constants on the card, 2 MB sections
  - DLSS Super Resolution
  - P-core/E-core thread placement
  - high-performance GPU pick on Optimus laptops
  - frame interval and render scale
  - the watchdog switch (on by default)
  - write-combined copy-back skip
  - ApexIV correctness fixes
  - faster shader warm with a pipeline book

  The first run of the merged build shows about 1.8 fps early on, against a 0.5 fps median before.
- **2026-11-26:** Series S profile tested: about 11 GB of RAM before the scene loads, but the device is removed on the first real scene. Being debugged.
- **2026-11-26:** first full 20-minute run with no crash (the record before was 10 min 19 s). Memory-pressure sweeps added.
- **2026-11-26:** testing tools added: playtest runner, frame-time and memory breakdown, charts, and per-second GPU clock, temperature and power logging.
- **2026-11-26:** M1LK created from VIctory and the DexNom fork. Setup script, warm shortcut, keyboard and mouse controls, M1LK HUD.

## What's in M1LK (26 Nov 2026)

Everything here is opt-in unless marked as a default, so the upstream (including AMD) behaviour is unchanged.

| Area | Switch | What it does |
| --- | --- | --- |
| Stability | `VICTORY_NO_WATCHDOG` (on by default in the launcher) | Byte-checked patch that stops the title's watchdog from killing a slow frame. From VItoria-Spark. |
| RAM | `VICTORY_RAM_SWEEP` (default on), `VICTORY_WINDOW_BUDGET_MB`, `VICTORY_RAM_TRIM_MB` | Lets go of host copies that only the title still holds, and trims the working set under pressure. |
| RAM | `VICTORY_WINDOWS_BAR_MB=1536`, `VICTORY_WINDOWS_BAR_FREE_MB=512` | Puts title windows in card memory the CPU writes directly (Resizable BAR), instead of locked system RAM. |
| RAM | `VICTORY_PORT_CONSTANTS=1` | Meant to keep constant buffers on the card (about 170 MB). **Not effective yet:** our logs show it places nothing (`0 regions in place`) because a follow-up fix from cool_fox is still missing. |
| RAM | `VICTORY_SECTION_MB=2` | Physical memory in 2 MB sections (faster remaps). From ApexIV. |
| RAM / picture | `VICTORY_TEXTURE_MIP_DROP`, `_MIN_SIZE`, `_FLOOR`, `VICTORY_TEXTURE_MIP_CAP` | Drops the top texture levels to save memory. A temporary trade: it makes surfaces softer. |
| fps | `VICTORY_UPSCALER=dlss`, `VICTORY_DLSS=quality\|balanced\|performance` | DLSS Super Resolution in place of the title's FSR. NVIDIA's DLL is loaded at runtime from your own driver and a `dlss` folder; M1LK ships no NVIDIA code (see `docs/dlss.md` and `tools/Setup-Dlss.ps1`). |
| fps | `VICTORY_FAST_CORES` (default 2) | Puts our submit and relay threads on P-cores and shader compilers on E-cores (hybrid Intel CPUs). From cool_fox. |
| fps | `VICTORY_ADAPTER` (default: high-performance GPU) | Always picks the NVIDIA GPU on Optimus laptops, and logs which one it used. |
| fps | `VICTORY_FRAME_INTERVAL_MS`, `VICTORY_TITLE_FPS`, `VICTORY_RENDER_SCALE`, `VICTORY_OUTPUT` | Frame pacing, render scale and output size. From cool_fox. |
| fps | `VICTORY_SKIP_WC_BACK=1` | Skips copying back write-combined memory the CPU never reads. From VItoria-Spark. |
| Shaders | `VICTORY_WARM_LOADERS`, `VICTORY_PIPELINE_WORKERS`, pipeline book | Faster warm, and records draw states so the next warm can build the graphics pipelines ahead of time. |
| Correctness | `VICTORY_BC7_ROTATION`, `VICTORY_INDIRECT_ZERO_GARBAGE`, `VICTORY_SDWA_SEXT_16`, `VICTORY_SETTLE_ROOM` | Picture and stability fixes from ApexIV. |
| Debugging | `VICTORY_SUBMIT_PROF`, `VICTORY_LOG_THREAD`, `VICTORY_MEMORY_CATEGORIES`, HUD | Per-frame cost and memory breakdowns. |

**Testing tools** (`tools/testing`): `run-m1lk.ps1` runs a full playtest. It keeps the PC awake, takes screenshots when the picture changes, samples memory, paging and GPU clocks/temperature/power each second, and finishes with a chart. `breakdown.py` ranks where frame time and memory go (paging stalls, copies, GPU, copy-back) and compares two runs. `perf-chart.py` draws the fps, draw and memory chart.

**Known issues:** `VICTORY_UPSCALER=dlss` crashes at start-up (fix in progress). Hair can render as stray lines, which a fix is on the way for. About a third of draws are still skipped (no pipeline yet, or an unsupported indirect draw), which can make walls look see-through.

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

## Step-by-step setup

Follow these steps in order if you have never built a Rust program before. They take about 30 to 60
minutes, most of it downloads and the first compile.

1. **Check your PC.** You need Windows 10 or 11 (64-bit), an RTX 20-series or newer NVIDIA card (or an
   RX 6000-series or newer AMD card), at least 16 GB of RAM and about 15 GB of free disk space for the
   build. Update your graphics driver from NVIDIA's or AMD's website.
2. **Install Git.** Download it from <https://git-scm.com/download/win>, run the installer and keep the
   default options.
3. **Install Visual Studio Build Tools.**
   1. Download them from <https://visualstudio.microsoft.com/visual-cpp-build-tools/>.
   2. In the installer, tick **Desktop development with C++**.
   3. Make sure that **MSVC v143 x64/x86 build tools** and a **Windows 11 SDK** are ticked on the right.
   4. Click **Install** and wait. It is a few GB.
4. **Install Rust.**
   1. Download `rustup-init.exe` from <https://rustup.rs> and run it.
   2. Press **Enter** to accept the defaults.
   3. When it finishes, close the window.
5. **Download M1LK.**
   1. Open **PowerShell** from the Start menu.
   2. Run:

      ```powershell
      cd $HOME
      git clone https://github.com/3cvil/M1LK.git
      cd M1LK
      ```

6. **Run the setup script.** It installs the pinned Rust toolchain, fetches and checks the BCPack format
   table, builds M1LK, and then asks you to pick your title's executable:

   ```powershell
   .\Setup.cmd
   ```

   - The first build takes 10 to 30 minutes.
   - If it fails with an out-of-memory error, close other programs, run
     `$env:CARGO_BUILD_JOBS = '4'` and then run `.\Setup.cmd` again.
   - When asked, choose the executable from **your own** installed copy of the title. M1LK never
     downloads or ships it.
7. **Warm the shader cache (strongly recommended).**
   - From the Start menu, run **M1LK Warm**, or run `%LOCALAPPDATA%\M1LK\m1lk-warm.exe`.
   - Let it finish. It can take 10 to 40 minutes the first time, and it only needs to run again after an
     update.
   - Without it, most of the picture is missing or blocky for the first minutes, because shaders are
     still being translated.
8. **Before you play on a 16 GB machine:**
   - Close your browser and other large programs.
   - Plug a laptop in to power.
   - In Windows settings, set sleep to **Never** while plugged in.
9. **Play.**
   - From the Start menu, run **M1LK**, or run `%LOCALAPPDATA%\M1LK\m1lk-play.exe --no-rt`.
   - Accept the title's own terms screen if it shows one.
   - See [Controls](#controls) for the keys.
10. **If something goes wrong:**
    - Run `.\tools\collect-logs.ps1`. It gathers your specs and logs, with personal paths removed.
    - Open an issue and attach the file it makes (see [Reporting a problem](#reporting-a-problem)).

To update later, run `git pull` in the M1LK folder and then `.\Setup.cmd` again.

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

## Controls

XInput controllers (Xbox pads) and DualSense are supported and work without setup. With no controller
connected, the keyboard and mouse drive a stand-in Xbox pad while the title's window has focus, and the
title shows its own gamepad prompts.

| Keyboard / mouse | Pad button | Typical use |
| --- | --- | --- |
| W A S D | Left stick | Move |
| Mouse, or I J K L | Right stick | Look / camera |
| Left mouse button, or C | Right trigger (RT) | Accelerate / fire |
| Right mouse button, or Z | Left trigger (LT) | Brake / aim |
| Enter | A | Confirm |
| Backspace | B | Back / cancel |
| X | X | |
| Y | Y | |
| Q / E | Left / right bumper | |
| F / H | Left / right stick click | |
| Arrow keys | D-pad | Menu navigation |
| Esc | Menu | Pause menu |
| Tab | View | Map / view |

Tips:

- **The mouse cursor.** While mouse look is on, the cursor is held in the middle of the game window.
  Alt+Tab to another window to get it back, or set `VICTORY_MOUSE_LOOK=0` to turn mouse look off.
- **Sensitivity.** `VICTORY_MOUSE_SENSITIVITY=2` turns twice as fast (the default is 1).
- **Rebinding.** For example, `VICTORY_KEYBINDS=Space=X,LShift=A,Mouse4=LB`.

All names and settings are in [docs/keyboard-and-mouse.md](docs/keyboard-and-mouse.md).

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

In priority order (the [Progress](#progress) table has the latest numbers):

1. **Series S upscaler.** Series S now runs with a picture and menus (4.5 fps, 17.1 GB peak). The title's
   own console upscaler shaders don't translate yet, so a simple spatial upscaler stands in for them.
2. **RAM use.** Peak commit is about 21 GB on Series X and 17 GB on Series S. The goal is 13 GB or less.
3. **Frame rate on 8 GB NVIDIA cards.** The median is 3.5 fps on Series X with ray tracing off. The goal
   is 30 fps that looks good (frame generation is fine, but report real fps next to it).
4. **Load time.** It takes about 10 minutes to reach player control.
5. **Ray tracing on NVIDIA** (wave32/wave64 shader translation, BVH builder).

### Contributing with Claude Code (the fastest way to help)

You can point [Claude Code](https://docs.anthropic.com/en/docs/claude-code/overview), an AI coding agent
that runs in your terminal, at this repository. It reads the README, the issues and the code, builds
M1LK, and opens a pull request for you. You need a Claude subscription or an Anthropic API key.

**1. Install the tools (once).** Open PowerShell:

```powershell
winget install Git.Git GitHub.cli Rustlang.Rustup
irm https://claude.ai/install.ps1 | iex     # installs Claude Code
gh auth login                               # lets Claude open the pull request from your account
```

Also install Visual Studio Build Tools with the "Desktop development with C++" workload (see
[Requirements](#requirements)).

**2. Fork and clone.** Click **Fork** at the top of this page, then:

```powershell
gh repo clone <your-user>/M1LK
cd M1LK
```

Copy `bcpack-tables.bin` into `crates/victory-formats/src/` (see
[Separately supplied data](#separately-supplied-data)), or the build fails.

**3. Start Claude Code** in that folder:

```powershell
claude
```

**4. Paste this prompt.** Replace the two lines in angle brackets first. Everything else can stay as it
is.

```text
You are contributing to M1LK (https://github.com/3cvil/M1LK), a Rust Xbox Series compatibility layer
for Windows, tuned for low-end NVIDIA PCs (8 GB VRAM, 16 GB RAM). I have cloned my fork in this folder.

My PC: <GPU and VRAM, CPU, RAM, Windows version>
My focus (or "pick the top one"): <e.g. Series S black picture / RAM / frame rate / load time / ray tracing>

Work in this order:
1. Sources first. Read README.md fully (Progress, Test log, Changelog, "What we need help with most",
   settings), docs/, and the open issues and pull requests on 3cvil/M1LK:
   gh issue list -R 3cvil/M1LK ; gh pr list -R 3cvil/M1LK
   Do not start work that an open pull request already covers.
2. Pick ONE problem: my focus, or the highest item in "What we need help with most" that you can make
   measurable progress on. Say in two lines what you picked and why before you write code.
3. Find the cause in the code (crates/victory-gpu, victory-shader, victory-hle, victory-formats)
   before you change anything. If another fork of VIctory already fixed it, port that fix and credit
   it (license must be GPL-3.0-or-later compatible) instead of writing a new one.
4. Make the smallest correct change. Put new behaviour behind a VICTORY_* environment switch when it
   could change results on other hardware, and log once when it takes effect.
5. Hard rules:
   - Keep AMD paths working. Never remove or regress Radeon support; gate NVIDIA-only changes on what
     the device reports (CheckFeatureSupport, vendor), not on assumptions.
   - No game names, title files, keys, saves, screenshots or data derived from a title in code,
     comments, tests, commits or the pull request. Say "the title".
   - No new downloads, network calls, binaries, build.rs scripts or git dependencies.
   - Never commit NVIDIA or other vendor binaries (for example DLSS DLLs).
6. Verify: cargo fmt; cargo clippy --workspace --all-targets -- -D warnings; cargo test --workspace;
   cargo build --release -p victory --bins. Add a unit test for the logic you changed.
7. If I own the title and want to test in game, tell me the exact launch command and environment
   variables, and what number or picture to compare before and after (frame time, committed memory,
   VRAM). Wait for my results; never run the title yourself without asking me.
8. Open one focused pull request to 3cvil/M1LK with gh pr create: what was wrong, what changed, which
   switch controls it, test results, my hardware, and before/after numbers if I gave them. Remove user
   names, paths, IP addresses and e-mail addresses from anything you paste.

Keep me updated in short messages, and ask me only when a decision really needs me.
```

**5. Test it in game (optional but very useful).** If you own the title, follow
[Step-by-step setup](#step-by-step-setup), run the launch command Claude gives you, and paste the
numbers back. The HUD shows fps, draws and memory. Never share title files or game screenshots.

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

M1LK also ports work from these forks, credited in each commit:

- **DoubleGhosty/cool_fox**: renderer port 1 (constants on the card), frame interval, render scale, hybrid-CPU placement.
- **martindevbr/VItoria-Spark**: watchdog patch, write-combined copy-back skip, card room for render targets.
- **KevinPratap/ApexIV**: memory sections, mip-drop limits, BC7, indirect and SDWA fixes, and the DLSS backend design.
- **Dolberthi/neptune-dolberth-nvidia-8gb**: lock-free write watch, submit profiler, log thread.
- The Resizable BAR window placement follows an earlier upstream change ("Card memory the CPU maps").

The full upstream commit history and author list are available in the upstream repositories linked above.

## License

GPL-3.0-or-later, the same as upstream VIctory. See [LICENSE](LICENSE). The separately supplied data
listed above is not covered by this license and is not distributed here.
