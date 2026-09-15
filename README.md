# koboterm

A terminal for Kobo e-readers. Tap a machine, get a shell on the e-ink screen,
type on an on-screen keyboard, and run things like Claude Code inside tmux
without the session ever depending on the Kobo staying awake.

Built and tested on a Kobo Clara BW. Other Kobos with recent firmware should
work; the panel driver comes from FBInk, which knows the whole lineup.

Status: early. It works end to end on one device. No installer yet, no
Bluetooth keyboard yet, no auto-reconnect yet.

## What it does

- Connects over SSH with a key that lives on the device. Nothing to type.
- Renders a real VT100 terminal on e-ink with a refresh scheduler tuned for
  streaming output: fast, flash-free partial updates, a full clean when idle.
- On-screen keyboard with one-shot Shift, Ctrl and Alt. Tap the terminal to hide
  or show it. Swipe to scroll.
- Home screen with your machines. Three terminal text sizes.
- Pairs a machine with one command run on that machine. The Kobo serves the
  setup script itself.
- Runs on top of the stock Kobo software. The Kobo UI is frozen while koboterm
  is up and thawed when you quit, so it resumes instantly.

## Install

There is no packaged installer yet. Today's route is for developers:

1. On the Kobo, install [NickelMenu](https://pgaskin.net/NickelMenu/).
2. Enable the stock SSH server: rename `.kobo/ssh-disabled` to
   `.kobo/ssh-enabled` on the Kobo's USB volume and reboot. The first
   `ssh root@<kobo-ip>` asks you to set a password.
3. Build the binary (see below) and copy it with the launcher:
   ```sh
   tools/kobo-push target/armv7-unknown-linux-musleabihf/release/koboterm /mnt/onboard/.adds/koboterm/koboterm
   tools/kobo-push host/kobo/koboterm.sh /mnt/onboard/.adds/koboterm/koboterm.sh
   ```
4. Add this line to `.adds/nm/config` on the Kobo:
   ```
   menu_item:main:koboterm:cmd_spawn:quiet:/mnt/onboard/.adds/koboterm/koboterm.sh
   ```

Then open NickelMenu and tap koboterm. The first launch generates the device's
SSH key.

### Pair a machine

The koboterm home screen shows one line. Run it on the machine you want to
reach, macOS or Linux:

```sh
curl -fsSL http://<kobo-ip>:8080/install.sh | sh
```

It authorizes the Kobo's key, turns on Remote Login on macOS, installs tmux,
starts a `kobo` tmux session at login, and registers the machine on the Kobo.
The machine appears on the home screen; tap it. Sessions attach to that tmux
session, so closing the Kobo never kills what you are running.

The tmux session is started inside your macOS login session on purpose: tools
that read the Keychain, such as Claude Code's login, work there and do not over
a bare SSH shell.

## Build

Rust stable, plus `zig` and `cargo-zigbuild` for the device target. No Docker.

```sh
brew install rustup zig && rustup toolchain install stable
rustup target add armv7-unknown-linux-musleabihf
cargo install cargo-zigbuild
git submodule update --init            # FBInk
cargo test                              # host-side tests
cargo zigbuild --release --target armv7-unknown-linux-musleabihf
```

The device binary is static musl, about 2.4 MB, and runs on the Kobo's ancient
glibc without any compatibility work.

`tools/kobo-sh` runs a script on the Kobo and `tools/kobo-push` copies a file
to it; both work around the stock sshd, which only serves interactive shells
and ships no scp.

## How it is put together

Small crates behind small interfaces, so most of it is tested on a laptop:

| crate | role |
|---|---|
| `term` | VT parser to a cell grid, DEC line drawing, scrollback, mouse reporting |
| `render` | the refresh scheduler: debounce with a latency cap, dirty row bands, ghost budget, heartbeat |
| `panel` | the drawing surface trait and a fake panel that counts refreshes |
| `panel-fbink` | the real e-ink panel through FBInk |
| `font` | BDF parser, ships Spleen |
| `input` | evdev touch, exclusive grab |
| `transport` | pty and in-process SSH (russh), key generation |
| `ui` | on-screen keyboard, home screen, add-machine form |
| `koboterm` | the app, the pairing server, Nickel freeze/thaw |

`AGENTS.md` has the details, including everything learned about the device.

## Credits

- [FBInk](https://github.com/NiLuJe/FBInk) by NiLuJe drives the e-ink panel.
- [Spleen](https://github.com/fcambus/spleen) by Frederic Cambus is the font.
- [KOReader](https://koreader.rocks) showed how to step around Nickel safely.
- [NickelMenu](https://pgaskin.net/NickelMenu/) by Patrick Gaskin is the launcher.

## License

GPL-3.0-or-later, because FBInk is linked in. See `LICENSE`.
