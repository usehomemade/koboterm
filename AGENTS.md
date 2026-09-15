# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

koboterm — an SSH terminal client for Kobo e-readers. Sit with a Kobo, connect to a remote machine, and work in a terminal (the primary use case is running Claude Code over SSH). `CLAUDE.md` is a symlink to this file; edit `AGENTS.md`.

**Status: greenfield.** No build, lint or test tooling exists yet. Fill in the Commands section when the toolchain lands; do not invent commands before then.

## Commands

_None yet._ Record here, once they exist: build the device binary, run host-side tests, run a single test, deploy to a Kobo.

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
