# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

koboterm — an SSH terminal client for Kobo e-readers. Sit with a Kobo, connect to a remote machine, and work in a terminal (the primary use case is running Claude Code over SSH). `CLAUDE.md` is a symlink to this file; edit `AGENTS.md`.

**Status: remote shell works, no input yet.** `koboterm ssh user@host` opens an SSH session from the Kobo and renders it on the e-ink screen (67x45 cells at Spleen 16x32 on the Clara BW); `koboterm run -- CMD` does the same for a local pty. Keystrokes can only be injected with `--type` so far: no keyboard, no launcher, no reconnect, no installer. License is GPL-3.0-or-later because FBInk is linked in.

## Commands

Toolchain: Rust stable via `rustup` (Homebrew), `zig` + `cargo-zigbuild` for cross-compiling. Target is `armv7-unknown-linux-musleabihf`, fully static, so the Kobo's old glibc is irrelevant.

```sh
cargo test                                   # host-side tests (terminal model, renderer, scheduler)
cargo test -p render streaming               # one test by name, in one crate
cargo run -p koboterm -- probe               # host: prints uname; on device also fb + input info
cargo zigbuild --release --target armv7-unknown-linux-musleabihf   # device binary (static, ~750 KB; builds FBInk via make)
tools/kobo-push target/armv7-unknown-linux-musleabihf/release/koboterm /tmp/koboterm   # deploy (about 1 min)
echo "/tmp/koboterm probe" | tools/kobo-sh                                              # run on device
echo "/tmp/koboterm demo" | tools/kobo-sh                                               # scripted session on the e-ink screen
echo "/tmp/koboterm keygen" | tools/kobo-sh                                             # device key -> /mnt/onboard/.adds/koboterm/id_ed25519
printf '/tmp/koboterm ssh tunc@192.168.0.199 --type "uname -a\\n" --type "exit\\n"\n' | tools/kobo-sh   # remote session (Mac with Remote Login on)
```

`cargo` lives in `/opt/homebrew/opt/rustup/bin`; add that to PATH.

Dev device access: stock Kobo firmware ships `.kobo/ssh-disabled`; renaming it to `ssh-enabled` and rebooting gives `ssh root@<ip>` (first login asks to set a password; then add a key to `/.ssh/authorized_keys`, root's home is `/`). The device's sshd ignores non-interactive commands and has no scp/sftp, hence `tools/kobo-sh` and `tools/kobo-push`. The Kobo drops Wi-Fi when it sleeps; wake it before connecting. This is a development convenience only. End-user install must be a `KoboRoot.tgz` dropped into `.kobo/` over USB (the same route NickelMenu and KOReader use), with zero manual steps.

## Dev device facts (Kobo Clara BW, model 395)

Verified 2026-09-15 by running `koboterm probe` on the device:

- Kernel 4.9.77, `armv7l`, 32-bit userland, glibc 2.11.1 (ancient: static musl is the right call). MediaTek MT8110 "TPV board". 448 MB RAM. Root fs 1 GB with 720 MB free; `/mnt/onboard` is the 12 GB FAT user partition.
- Framebuffer `/dev/fb0`: driver `hwtcon` (MediaTek, not mxcfb), 1072x1448, 32 bpp, rotate=3, line_length 4288. FBInk knows this driver; do not hand-roll its ioctls.
- Input: `event0` gpio-keys, `event1` cyttsp5_mt (touch), `event2` power key. No keyboard devices until OTG/Bluetooth work.
- Present: busybox, wpa_supplicant, dhcpcd, hciconfig, OpenSSH 8.9 sshd. Missing: ssh client, scp, sftp-server, tmux, bluetoothd.
- Running: `nickel` (the reader UI), `hindenburg`, `sickel`, `fontickel`, `strickel`, `wmt_launcher` (MediaTek Wi-Fi/BT combo driver daemon), `mdpd`.

## Crates

- `crates/panel` — `Panel` trait (cell-addressed draw + refresh with a `Waveform`), `FakePanel` that logs refreshes for tests.
- `crates/term` — terminal model over `vt100`; `Terminal::snapshot()` returns a `Grid` of reduced cells.
- `crates/render` — the refresh scheduler. Diffs wanted grid vs shadow grid on every tick; debounce with a latency cap; row-band coalescing; ghost budget. Tests here are the "refreshes per second" metric.
- `crates/font` — BDF parser; ships Spleen 16x32 and 12x24 (BSD-2-Clause). Glyphs the font lacks render as a hollow box.
- `crates/fbink-sys` — raw FFI to FBInk. `build.rs` copies `third_party/FBInk` (git submodule, pinned) into `OUT_DIR` and runs its Makefile (`staticlib KOBO=true MINIMAL=true DRAW=1`) with the compiler cargo resolved. Bindings are pre-generated with bindgen for the ARM target and committed. Empty crate on non-Linux hosts.
- `crates/panel-fbink` — `FbinkPanel: Panel`. Writes glyphs straight into FBInk's mapped framebuffer, maps `Waveform` to DU / GL16 / GC16, refreshes via `fbink_refresh`. Linux only.
- `crates/transport` — `Transport` trait; `PtyTransport` (forkpty, host-tested) and `SshTransport` (russh in-process, pubkey auth, pty + shell/exec, resize; tokio runtime on a background thread so the main loop stays synchronous). `keygen` writes an OpenSSH ed25519 key pair. Host keys are not verified yet.
- `crates/koboterm` — the binary: `probe`, `demo`, `keygen`, `run -- CMD` and `ssh user@host[:port] [--key PATH] [--cmd COMMAND]`. `--type TEXT` injects keystrokes 1 s apart, `--hold SECS` keeps the last screen after exit. The session loop in `main.rs` (`session::run`) is the shape of the real app: read transport, feed terminal, tick renderer every 10 ms.

Rebuilding the bindings after an FBInk bump: preprocess with `zig cc -target arm-linux-musleabihf -DFBINK_FOR_KOBO -DFBINK_MINIMAL -DFBINK_WITH_DRAW -E -P third_party/FBInk/fbink.h`, run `bindgen` on the result with `-- -target arm-linux-musleabihf` (needs `LIBCLANG_PATH=/Library/Developer/CommandLineTools/usr/lib`), allowlist `fbink_.*` functions and `FBInk.*`/`*_INDEX_[TE]` types, `--no-layout-tests --use-core`.

## The idea

A Kobo is a Linux ARM box with Wi-Fi and an e-ink framebuffer. koboterm turns it into a dedicated remote terminal: one tap opens a session to a configured host, a keyboard types into it, and the screen shows a terminal that stays readable despite e-ink's refresh behaviour. It is a native app on the device, not a plugin inside another reader.

## Architecture

Five layers, each behind a small interface so they can be developed and tested independently on a laptop before touching the device.

1. **Transport** — opens and keeps a session to the remote host. Must survive Wi-Fi sleep and reconnect without losing the session; this is a hard requirement, since Kobos drop Wi-Fi aggressively. Design the interface so a reconnecting transport (mosh-style) and a plain ssh transport are interchangeable.
2. **Terminal model** — parses the byte stream from the transport into a cell grid: characters, attributes, cursor, scrollback. Pure and host-testable. Emits *dirty regions*, not "redraw everything".
3. **Renderer** — turns dirty regions into e-ink framebuffer updates. This is where the project earns its keep (see constraints below). Owns the refresh scheduler.
4. **Input** — collects keystrokes from whichever sources are available and feeds them to the transport. Sources, in priority order: Bluetooth keyboard, on-screen keyboard, USB OTG serial. Each source is a separate module behind one interface.
5. **Shell** — launcher, host config, session lifecycle, and the device integration needed to start from the Kobo home screen and hand the screen back cleanly on exit.

Host-side simulator: the terminal model and renderer should run on a laptop against a fake framebuffer so rendering and refresh-scheduling logic can be iterated without a device in hand.

## Design constraints

These drive every rendering and networking decision.

- **E-ink refresh is the whole problem.** Full refreshes are slow and flash; partial refreshes ghost. Streaming output and spinners (Claude Code does both constantly) will hammer a naive implementation. The renderer must batch and debounce redraws, refresh only dirty regions, coalesce rapid updates to the same region, and schedule periodic full refreshes to clear ghosting. Treat "refreshes caused per second of output" as a first-class metric with a test.
- **Sessions must outlive Wi-Fi.** Reconnect transparently; never make the user re-login after the device naps.
- **Input is scarce and slow.** Bluetooth keyboard is the target experience. The on-screen keyboard is a fallback, not the plan.
- **One tap to a prompt.** Launch, connect to the default host, show a shell. Configuration lives in a file, not a settings UI, at least initially.
- **Readability over fidelity.** Prefer a large, clear monospace font and a conservative colour-to-grey mapping over exact terminal colours.
