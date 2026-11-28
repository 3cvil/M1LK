# Test charts

One chart per test run, an overview across all tests and an overall score. Generated from the test log by `test-charts.py`; the numbers for every test are in [tests.csv](tests.csv).

![Overview](overview.svg)

![Score](score.svg)

## How the score is computed

Each part is scaled 0 to 100 against the project goal, then combined with these weights. If a part has no data for a test, it is left out and its weight is spread over the other parts (hollow marker on the score chart).

| Part | Weight | 100 points when | 0 points when |
|---|---|---|---|
| Walking fps | 35% | median walking fps is 30 or more | 0 fps |
| Standing fps | 5% | median standing fps is 30 or more | 0 fps |
| Smoothness | 5% | walking low equals walking median | walking low is 0 |
| Peak RAM | 15% | peak process RAM is 13 GB or less | peak is 20 GB or more |
| Stability | 15% | run reached its planned end | crash or black screen (50 for a watchdog or first-run stop) |
| Picture | 10% | clean (minor 60, artifacts or blocks 30) | broken (black or scrambled) |
| Lighting / RT | 10% | ray tracing on (50) plus normal picture brightness (50) | ray tracing off and a near-black or blown-out picture |
| Load time | 5% | first world frame within 60 s | 600 s or more |

Lighting / RT: 50 points if ray tracing was on (read from the run mode; every test so far ran with it off), plus up to 50 for picture brightness: the median mean luminance (0 to 255) of in-world screenshots taken after the load time while drawing more than 200 draws per frame. 50 points when that median is 70 to 140, falling linearly to 0 at 10 or below and at 230 or above. This is only a rough proxy for lighting quality.

## Tests

| Test | Date | Change | Standing fps | Walking fps | Peak RAM (GB) | Load (s) | Picture | Score | Chart |
|---|---|---|---|---|---|---|---|---|---|
| 1 | 2026-11-26 | memory fix 1 | - | - | 16.8 | - | unchecked | - | [chart](test-01.svg) |
| 2 | 2026-11-26 | RAM branch | 0.1 | - | 19.6 | 203 | unchecked | 19* | [chart](test-02.svg) |
| 3 | 2026-11-26 | RAM branch + pressure sweeps | 0.1 | 0.4 | 21.1 | 209 | unchecked | 28* | [chart](test-03.svg) |
| 4 | 2026-11-26 | merged build (all of today's work) | 2.7 | 2.7 | 19.6 | 150 | unchecked | 32* | [chart](test-04.svg) |
| 5 | 2026-11-26 | + skip write-combined copy-back, + window tails | 0.7 | 3.2 | 20.1 | 140 | unchecked | 30* | [chart](test-05.svg) |
| 6 | 2026-11-26 | new build: hair skip, UI mips kept, quiet-copy skip, sliced heap resc... | 3.2 | 3.4 | 21.7 | 173 | unchecked | 35* | [chart](test-06.svg) |
| 7 | 2026-11-26 | + wide refresh | 3.3 | 3.5 | 21.3 | 155 | unchecked | 34* | [chart](test-07.svg) |
| 8 | 2026-11-26 | Series S fix | 4.3 | - | 16.8 | 141 | unchecked | 34* | [chart](test-08.svg) |
| 9 | 2026-11-26 | Series S fix | 5.1 | 5 | 17.3 | 157 | black picture | 35 | [chart](test-09.svg) |
| 10 | 2026-11-26 | Series S picture fix | 3.8 | 4.2 | 17.1 | 155 | shows, very dark | 41 | [chart](test-10.svg) |
| 11 | 2026-11-27 | + ambient light stand-in, title's own upscaler | 3.7 | 4.5 | 16.4 | 234 | white blocks/specks | 40 | [chart](test-11.svg) |
| 12 | 2026-11-27 | same build, repeat | 3.9 | 5 | 16.9 | 170 | white blocks/specks | 40 | [chart](test-12.svg) |
| 13 | 2026-11-27 | DLSS fix, copy changes, fork fixes | - | - | 5.8 | - | not valid | - | [chart](test-13.svg) |
| 14 | 2026-11-27 | fork fixes (shader NaN, helper lanes, cache, compute waits) + DLSS fix | 3.5 | 5.4 | 16.9 | 190 | clean, faint smear | 44 | [chart](test-14.svg) |
| 15 | 2026-11-27 | fork fixes, shader loop limit 1024 (was 32) | 3.3 | - | 16.8 | 192 | white blocks/specks | 38* | [chart](test-15.svg) |
| 16 | 2026-11-27 | fork fixes, title memory kept in place (M1LK_INPLACE=1) | 4 | 2.6 | 16.8 | 167 | clean | 43 | [chart](test-16.svg) |
| 17 | 2026-11-27 | parallel texture checks (M1LK_PARALLEL_REFRESH=1) | - | - | 5.3 | - | unchecked | - | [chart](test-17.svg) |
| 18 | 2026-11-27 | parallel texture checks (M1LK_PARALLEL_REFRESH=1), rerun | 0.7 | 1.1 | 16.6 | 366 | dark, no artifacts named | 35 | [chart](test-18.svg) |
| 19 | 2026-11-27 | fill light 0.6 (M1LK_AMBIENT=0.6; default 0.3) | 4 | 3.8 | 16.9 | 178 | dark, no artifacts named | 40 | [chart](test-19.svg) |
| 20 | 2026-11-27 | fill light follows the sky (M1LK_AMBIENT=sky): picture brightness che... | 4 | 3.9 | 16.5 | 142 | clean | 48 | [chart](test-20.svg) |
| 21 | 2026-11-27 | shader loop cap 1024 instead of 32 (dark picture fix test) | 4.4 | 3.7 | 15.8 | 184 | clean | 46 | [chart](test-21.svg) |
| 22 | 2026-11-27 | in-place constants heap (M1LK_INPLACE_CONSTANTS=1, port constants off) | 4.9 | 4.1 | 16.7 | 151 | clean | 31 | [chart](test-22.svg) |
| 23 | 2026-11-27 | write-watch asked once per step (M1LK_ASK_REASK=1) | 4 | 3.6 | 16.6 | 207 | clean | 46 | [chart](test-23.svg) |
| 24 | 2026-11-27 | in-place constants rerun after the arrange fix | 3.4 | 3.8 | 16.3 | 189 | clean | 47 | [chart](test-24.svg) |
| 25 | 2026-11-27 | GPU detile + in-place constants (fps) | 1.3 | 2 | 17.1 | 185 | scrambled textures | 31 | [chart](test-25.svg) |
| 26 | 2026-11-27 | plugged-in baseline with the submit profiler (the copy profiler was n... | 2.6 | 3.4 | 17 | 171 | clean, faint smear | 39 | [chart](test-26.svg) |
| 27 | 2026-11-27 | batch the per-record indirect clamps (about 2,700 extra GPU passes a... | 4.4 | 4.2 | 17 | 151 | clean | 47 | [chart](test-27.svg) |
| 28 | 2026-11-27 | copy time breakdown (profiling run, no behavior change) | 4.4 | 4.3 | 16.9 | 136 | yard blocks | 41 | [chart](test-28.svg) |
| 29 | 2026-11-27 | in-place host reads instead of a kernel call per copy (ported) | 4.5 | 3 | 17.2 | 149 | yard blocks | 38 | [chart](test-29.svg) |
| 30 | 2026-11-27 | CGBJ low-commit back-off: defer texture uploads and cap look-ahead st... | 4.8 | 4.3 | 17.2 | 139 | yard blocks | 41 | [chart](test-30.svg) |
| 31 | 2026-11-27 | yard-block check: exact wave64 split instead of the per-half approxim... | 4.3 | 4.1 | 16.7 | 148 | yard blocks | 40 | [chart](test-31.svg) |
| 32 | 2026-11-27 | check: start from the cell autosave (pair runs share this start point... | 3.8 | 3.8 | 16.9 | 127 | unchecked | 42* | [chart](test-32.svg) |
| 33 | 2026-11-27 | copy-plain + remap lock ON (its off pair was dropped; round 1 retests... | 3.8 | 4.3 | 16.1 | 138 | clean, faint smear | 45 | [chart](test-33.svg) |
| 34 | 2026-11-27 | round 1 (combo a56b129): baseline A, every switch off | 4.4 | 4.5 | 17 | 156 | floating box, dither | 44 | [chart](test-34.svg) |
| 35 | 2026-11-27 | round 1: ONE change on vs baseline: host copies read title memory in... | 4.3 | 4.5 | 17.4 | 136 | clean, faint smear | 42 | [chart](test-35.svg) |
| 36 | 2026-11-27 | round 1: ONE change on vs baseline: remap lock released right after t... | 4.2 | 4.1 | 17.1 | 143 | dark, no artifacts named | 44 | [chart](test-36.svg) |
| 37 | 2026-11-27 | round 1: ONE change on vs baseline: wider write-watch asks, one scan... | - | - | 3.7 | - | unchecked | - | [chart](test-37.svg) |
| 38 | 2026-11-27 | round 1: ONE change on vs baseline: texture upload back-off under low... | 4.5 | 4.7 | 16.6 | 138 | dark, no artifacts named | 44 | [chart](test-38.svg) |
| 39 | 2026-11-27 | round 1: ONE change on vs baseline: constants heap in place | 4 | 4.4 | 16.9 | 144 | see README | 41* | [chart](test-39.svg) |
| 40 | 2026-11-27 | round 1: ONE change on vs baseline: texture detiling on the card | 2.2 | 4.3 | 18.7 | 160 | see README | 36* | [chart](test-40.svg) |
| 41 | 2026-11-27 | CPU sampling profiler (where the ~215 ms frame goes, per thread and f... | 4.1 | 4.4 | 16.8 | 168 | unchecked | 43* | [chart](test-41.svg) |
| 42 | 2026-11-27 | VIctory-NVIDIA snapshot (test/vnv-base), frame generation off, same s... | - | - | 5.4 | - | unchecked | - | [chart](test-42.svg) |
| 43 | 2026-11-27 | VIctory-NVIDIA snapshot (test/vnv-base) rerun with a log file, frame... | - | - | 5.3 | - | unchecked | - | [chart](test-43.svg) |
| 44 | 2026-11-27 | VIctory-NVIDIA snapshot, now with our save and settings copied into i... | 1.5 | 1.5 | 16.6 | 136 | white blocks/specks | 23* | [chart](test-44.svg) |
| 45 | 2026-11-27 | VIctory-reloaded snapshot 9941051 (its own NVIDIA defaults): whole-bu... | 0.3 | 3.3 | 16.6 | 130 | dark, no artifacts named | 42 | [chart](test-45.svg) |
| 46 | 2026-11-27 | VIctory-reloaded snapshot, second (warm) run: shaders/pipelines alrea... | - | - | 13.1 | - | unchecked | - | [chart](test-46.svg) |
| 47 | 2026-11-27 | round 1 baseline B repeat (current main build, profilers on) | 4.1 | 4.2 | 16.9 | 141 | see README | 45* | [chart](test-47.svg) |
| 48 | 2026-11-27 | pair for r12-homogenize-on (same build, M1LK_INPLACE off) | 1.1 | 4.3 | 17.2 | 153 | unchecked | 41* | [chart](test-48.svg) |
| 49 | 2026-11-27 | M1LK ZCM diagnosis: logs why the constants-heap arrange step exits si... | 1.3 | 4.5 | 17.2 | 151 | unchecked | 41* | [chart](test-49.svg) |
| 50 | 2026-11-27 | ASK_REASK control (same build, switch off): biggest estimated CPU lev... | 1.1 | 4.2 | 17.2 | 141 | unchecked | 43* | [chart](test-50.svg) |
| 51 | 2026-11-27 | ASK_REASK on: stop re-asking GetWriteWatch each flush | 1.1 | 3.9 | 17.6 | 140 | unchecked | 38* | [chart](test-51.svg) |
| 52 | 2026-11-27 | M1LK ZCM fix 650f79b8: the 32-bit graphics heap's reservation recorde... | 1.4 | 5 | 18.3 | 142 | unchecked | 38* | [chart](test-52.svg) |
| 53 | 2026-11-27 | M1LK ZCM combo build (round12 + heap fix 82ffb0bd), both zero-copy sw... | 1.1 | 3.8 | 17.1 | 154 | unchecked | 40* | [chart](test-53.svg) |
| 54 | 2026-11-27 | M1LK ZCM combo build, both zero-copy paths ON: constants heap (174 MB... | 1.6 | 4.4 | 16.7 | 158 | unchecked | 44* | [chart](test-54.svg) |
| 55 | 2026-11-27 | M1LK ZCM Tell (df400589): combo-on plus telling placed textures what... | 3.2 | 4.2 | 16.1 | 142 | unchecked | 46* | [chart](test-55.svg) |
| 56 | 2026-11-27 | M1LK ZCM combo-on without the 2 MB section split: does the section-vi... | 1.6 | 4 | 17.2 | 137 | unchecked | 40* | [chart](test-56.svg) |
| 57 | 2026-11-27 | frame-overlap quick test, existing switches only: combo-on plus more... | 1.6 | 4.3 | 17.4 | 132 | unchecked | 40* | [chart](test-57.svg) |
| 58 | 2026-11-27 | M1LK Watch Skip pair, OFF half (same exe as ws-on | 1.6 | 4.1 | 17.1 | 146 | unchecked | 42* | [chart](test-58.svg) |
| 59 | 2026-11-27 | M1LK Watch Skip ON: skip ~4150 failed write-watch asks/frame on view... | 1.6 | 4.2 | 17.3 | 138 | unchecked | 41* | [chart](test-59.svg) |
| 60 | 2026-11-27 | M1LK Strain: let 4 non-hair programs (cull masks, LOD blends, decal c... | 1.5 | 4.4 | 17.1 | 144 | unchecked | 41* | [chart](test-60.svg) |
| 61 | 2026-11-27 | M1LK Strain OFF half (same exe as strain) | 1.5 | 3.9 | 17.2 | 129 | unchecked | 40* | [chart](test-61.svg) |
| 62 | 2026-11-27 | ladder step 2: Watch Skip (repeat of ws-on, curd exe) | 1.8 | 4.3 | 16.4 | 139 | unchecked | 42* | [chart](test-62.svg) |
| 63 | 2026-11-27 | ladder step 1: none (Watch Skip repeat OFF half, curd exe) | 2.1 | 4.2 | 16.6 | 158 | unchecked | 42* | [chart](test-63.svg) |
| 64 | 2026-11-27 | ladder step 3: Watch Skip + COPY_PLAIN | 1.6 | 4.3 | 16.8 | 138 | unchecked | 42* | [chart](test-64.svg) |
| 65 | 2026-11-27 | ladder step 4: Watch Skip + COPY_PLAIN + M1LK Curd (constant uploads... | 1.8 | 3.8 | 16.8 | 139 | unchecked | 43* | [chart](test-65.svg) |
| 66 | 2026-11-27 | profiling run: submit-path time split into lock waits, card waits, bu... | 2.4 | 4 | 16.9 | 150 | unchecked | 40* | [chart](test-66.svg) |
| 67 | 2026-11-27 | rounds 2+4 in one run: 9 changes (incl. batched write-watch asks) swi... | 1.9 | 4.5 | 16.6 | 135 | unchecked | 43* | [chart](test-67.svg) |
| 68 | 2026-11-27 | round 6: lock-free nothing-new check, all table slots skipped by the... | 1.5 | 4.2 | 16.8 | 146 | unchecked | 44* | [chart](test-68.svg) |
| 69 | 2026-11-27 | M1LK Skimmer ON (held lines on a dedicated thread), A/B 1 | 2.1 | 4.7 | 16.7 | 145 | unchecked | 42* | [chart](test-69.svg) |
| 70 | 2026-11-27 | M1LK Skimmer OFF, same exe, A/B 1 | 2.5 | 4 | 16.8 | 128 | unchecked | 42* | [chart](test-70.svg) |
| 71 | 2026-11-27 | M1LK Skimmer ON, A/B 2 | 1.9 | 4.3 | 16.8 | 133 | unchecked | 43* | [chart](test-71.svg) |
| 72 | 2026-11-27 | M1LK Pour on (8 runs per chunk), A/B 1 | 1.8 | 3.8 | 16.7 | 136 | unchecked | 41* | [chart](test-72.svg) |
| 73 | 2026-11-27 | M1LK Pour off, same exe, A/B 1 (also first Lactometer step timings) | 2.1 | 4 | 16.8 | 141 | unchecked | 42* | [chart](test-73.svg) |
| 74 | 2026-11-27 | existing parallel prepare switches all on (wide refresh, parallel tex... | 2.4 | 4.2 | 16.4 | 136 | unchecked | 44* | [chart](test-74.svg) |
| 75 | 2026-11-27 | RAM: smaller host-copy windows (1 MiB grid) + 1.5 GB window budget vs... | 2.5 | 4 | 17.1 | 130 | unchecked | 43* | [chart](test-75.svg) |
| 76 | 2026-11-27 | RAM: texture budget capped at 3.5 GB vs pour-off same exe | 2 | 4 | 17.3 | 143 | unchecked | 42* | [chart](test-76.svg) |
| 77 | 2026-11-27 | STACK on: Rennet + COPY_PLAIN + Curd, A/B 1 - does a big CPU-prep cut... | 1.4 | 4 | 16.8 | 142 | unchecked | 42* | [chart](test-77.svg) |
| 78 | 2026-11-27 | STACK off, same exe, A/B 1 - does a big CPU-prep cut move fps? | 1.6 | 4 | 16.4 | 146 | unchecked | 42* | [chart](test-78.svg) |
| 79 | 2026-11-27 | STACK on, A/B 2 - does a big CPU-prep cut move fps? | 2.1 | 4.2 | 16.8 | 142 | unchecked | 42* | [chart](test-79.svg) |
| 80 | 2026-11-28 | STACK off, A/B 2 - does a big CPU-prep cut move fps? | 1.1 | 3.8 | 17.6 | 140 | unchecked | 40* | [chart](test-80.svg) |
| 81 | 2026-11-28 | M1LK Churn Clock: GPU timestamps on host queue (stack on) | 1.4 | 4.3 | 16.9 | 140 | unchecked | 42* | [chart](test-81.svg) |
| 82 | 2026-11-28 | RAM: M1LK Skim Milk on (title heaps committed on first touch), A/B 1 | 2 | 4.1 | 16.7 | 133 | unchecked | 43* | [chart](test-82.svg) |
| 83 | 2026-11-28 | Churn Clock at 1280x720 output (render ~853x480): is card exec pixel-... | 1 | 4.5 | 17 | 139 | unchecked | 41* | [chart](test-83.svg) |
| 84 | 2026-11-28 | Churn Lists: per-command-list GPU time, top lists | 1.5 | 5.5 | 17.3 | 142 | unchecked | 33* | [chart](test-84.svg) |
| 85 | 2026-11-28 | RAM: M1LK Skim Milk off, same exe, A/B 1 | 1.4 | 4.4 | 16.8 | 136 | unchecked | 41* | [chart](test-85.svg) |
| 86 | 2026-11-28 | RAM: M1LK Skim Milk on (title heaps committed on first touch), A/B 1 | 2.2 | 4.5 | 16.5 | 133 | unchecked | 45* | [chart](test-86.svg) |
| 87 | 2026-11-28 | RAM: M1LK Skim Milk on (title heaps committed on first touch), A/B 1 | 2 | 4.4 | 16.5 | 133 | unchecked | 43* | [chart](test-87.svg) |
| 88 | 2026-11-28 | barrier A/B baseline: CHURN_CLOCK host_exec, BARRIER_TEST off | 1.8 | 4.7 | 16.7 | 138 | unchecked | 43* | [chart](test-88.svg) |
| 89 | 2026-11-28 | barrier A/B: drop global UAV waits (clamp+groups) | 1.9 | 4.8 | 17.9 | 145 | unchecked | 38* | [chart](test-89.svg) |
| 90 | 2026-11-28 | INDIRECT_WINDOW A/B baseline (clean exe, real fps+picture) | 1.5 | 4.3 | 15.5 | 139 | unchecked | 46* | [chart](test-90.svg) |
| 91 | 2026-11-28 | INDIRECT_WINDOW=1: clamp up to 64 records/pass | 2 | 4.2 | 17.5 | 134 | unchecked | 41* | [chart](test-91.svg) |
| 92 | 2026-11-28 | prep-parallel A/B baseline (title-thread critical-path test) | - | - | 22.6 | - | unchecked | - | [chart](test-92.svg) |
| 93 | 2026-11-28 | prep-parallel: move sweep off + parallel refresh/preludes | 1.1 | 4.1 | 16.4 | 149 | unchecked | 44* | [chart](test-93.svg) |
| 94 | 2026-11-28 | matched baseline for sweep-off (HEAP_LAZY=1 only) - isolates SWEEP ef... | 1.8 | 3.3 | 23.3 | 146 | unchecked | 30* | [chart](test-94.svg) |
| 95 | 2026-11-28 | CLEAN sweep-off only +HEAP_LAZY (offset RAM): fps vs pp-off 4.5? DIAG... | 1.4 | 4.2 | 15.4 | 146 | unchecked | 44* | [chart](test-95.svg) |
| 96 | 2026-11-28 | clamp-barrier A/B baseline (CLAMP_LOCAL_BARRIER off): host_exec+fps+p... | 2.6 | 3.8 | 17.7 | 138 | unchecked | 38* | [chart](test-96.svg) |
| 97 | 2026-11-28 | clamp-barrier FIX: scoped UAV barriers in clamp | 1.6 | 3.8 | 17.9 | 142 | unchecked | 38* | [chart](test-97.svg) |
| 98 | 2026-11-28 | SUBMIT_THREAD A/B baseline (worker off): fps+RAM+submit profile+host_... | 1.2 | 3.9 | 17.5 | 136 | unchecked | 39* | [chart](test-98.svg) |
| 99 | 2026-11-28 | * SUBMIT_THREAD=1: prep on worker off title thread. fps rise? title-t... | 1.8 | 4.2 | 16.9 | 144 | unchecked | 42* | [chart](test-99.svg) |
| 100 | 2026-11-28 | round 5: picture fixes ported from VIctory-reloaded and Vice-Tay (sha... | 2.3 | 4.1 | 16.6 | 147 | unchecked | 42* | [chart](test-100.svg) |
| 101 | 2026-11-28 | * TRANSLATE PROF: per-step CPU ground truth - which translation step... | 1.5 | 4.4 | 16.8 | 141 | unchecked | 43* | [chart](test-101.svg) |
| 102 | 2026-11-28 | heap-async A/B baseline (async off, tprof on): fps + heap_buffers buc... | 1.4 | 4.5 | 19.8 | 141 | unchecked | 37* | [chart](test-102.svg) |
| 103 | 2026-11-28 | * HEAP_PASS_ASYNC=1: drop buffer_pass lock wait. fps rise? heap_buffe... | 1.2 | 3.9 | 15.5 | 148 | unchecked | 45* | [chart](test-103.svg) |
| 104 | 2026-11-28 | RAM: M1LK Skim Milk on (title heaps committed on first touch), A/B 1 | 1.6 | 3.4 | 19.6 | 134 | unchecked | 31* | [chart](test-104.svg) |
| 105 | 2026-11-28 | RAM: M1LK Skim Milk on (title heaps committed on first touch), A/B 1 | 1.8 | 4.2 | 17.7 | 144 | unchecked | 41* | [chart](test-105.svg) |
| 106 | 2026-11-28 | GPU-bound test baseline (shader loops=32): host_exec+fps | 1.4 | 4.5 | 17.3 | 133 | unchecked | 41* | [chart](test-106.svg) |
| 107 | 2026-11-28 | * GPU-bound TEST: shader loops=4 (slash GPU shader work). host_exec d... | 1.5 | 4.3 | 16.4 | 132 | unchecked | 44* | [chart](test-107.svg) |
| 108 | 2026-11-28 | GPU-bound test milder (shader loops=16): host_exec drop + fps change... | 1.6 | 3.5 | 16.4 | 142 | unchecked | 43* | [chart](test-108.svg) |
| 109 | 2026-11-28 | round 7: landed command lists reset on the helper thread instead of i... | 0.1 | 0.1 | 16.8 | 184 | unchecked | 30* | [chart](test-109.svg) |
| 110 | 2026-11-28 | * TEXTURE USE: measure never-read texture MB (VRAM evictable lower bo... | 2 | 4.2 | 16.6 | 146 | unchecked | 42* | [chart](test-110.svg) |
| 111 | 2026-11-28 | * idle-split: free_to_drop vs held_by_slot of stale textures - decide... | 1.6 | 3.9 | 16.6 | 145 | unchecked | 39* | [chart](test-111.svg) |
| 112 | 2026-11-28 | repeat idle-split: confirm free_to_drop=0 / stale pinned (1 run so fa... | 1.4 | 3.9 | 16.3 | 152 | unchecked | 42* | [chart](test-112.svg) |
| 113 | 2026-11-28 | table-cache A/B baseline (hash swap off): descriptor_copy tprof + fps | 1.8 | 4.2 | 16.7 | 146 | unchecked | 42* | [chart](test-113.svg) |
| 114 | 2026-11-28 | TABLE_CACHE=1 (folded-multiply hash): descriptor_copy bucket drop? fp... | 1.3 | 3.7 | 16.4 | 145 | unchecked | 42* | [chart](test-114.svg) |
| 115 | 2026-11-28 | * descriptor_copy SPLIT: dyn_alloc(safe) vs make_at/textures(hard) -... | 2 | 4 | 16 | 141 | unchecked | 43* | [chart](test-115.svg) |
| 116 | 2026-11-28 | table-cache REPEAT on #2 (confirm fps +0.6 not noise) | 1.9 | 3.9 | 16.8 | 135 | unchecked | 41* | [chart](test-116.svg) |
| 117 | 2026-11-28 | table-cache REPEAT off #2 | 1.5 | 3.9 | 16.9 | 142 | unchecked | 41* | [chart](test-117.svg) |
| 118 | 2026-11-28 | * DECISIVE: mirror made_wait vs held vs work - does the biggest fps c... | 0.1 | 0.4 | 15.8 | 191 | unchecked | 35* | [chart](test-118.svg) |
| 119 | 2026-11-28 | repeat descriptor_copy split - confirm tbl_other dominates (ratios sw... | 0.1 | 0.2 | 16.6 | 195 | unchecked | 32* | [chart](test-119.svg) |
| 120 | 2026-11-28 | made fast-path A/B baseline (off): fps + made_wait | 1.4 | 3.8 | 17 | 146 | unchecked | 41* | [chart](test-120.svg) |

\* score computed without every part (see `score_missing_parts` in tests.csv).
