# screenshotr

A headless macOS service that captures the **active screen** (the display under
the mouse cursor) and returns it as JPEG. Drive it from a browser or over HTTP.

Runs as a signed, headless `.app` bundle started at login by a LaunchAgent.
Capture goes through ScreenCaptureKit — `CGDisplayCreateImage` is obsoleted in
macOS 15+ and will not compile against a current SDK.

- **Browser UI** at `http://<host>:8765/` — enter the token once, click to
  capture, view the shot inline, and open it full-screen to zoom and pan.
- **HTTP API** — `GET /screenshot` returns raw `image/jpeg` for scripting.

## Requirements

- macOS 14.0+ (developed and verified on macOS 26.5, Apple Silicon)
- Full Xcode — the `screencapturekit` crate compiles a Swift bridge via `xcrun`
- A code-signing identity (see [Signing](#signing))

## Install (this machine)

```sh
make install
```

This builds, signs, copies the bundle to `~/Applications/ScreenshotR.app`,
generates a token, installs the LaunchAgent, and starts it.

On first run the app has no Screen Recording permission, so it opens
**System Settings → Privacy & Security → Screen Recording** and exits. Toggle
**ScreenshotR** on; launchd restarts it within ~10s and it begins serving.

## Install on another Mac

```sh
make dist
```

Produces `dist/screenshotr-<version>-arm64.tar.gz` (~1.1 MB): the signed app
plus a standalone `install.sh`. No Xcode or Rust needed on the target — only
macOS 14+ on Apple Silicon.

**Transfer it without a quarantine flag.** `scp`, `rsync` and USB are clean:

```sh
scp dist/screenshotr-0.1.0-arm64.tar.gz othermac:~/
ssh othermac 'tar xzf screenshotr-0.1.0-arm64.tar.gz && ./screenshotr-0.1.0-arm64/install.sh'
```

The installer checks arch and macOS version, verifies the signature, installs
the app and LaunchAgent, generates a token (keeping any existing one), and waits
for the service to come up. Grant Screen Recording on that Mac the same way as
above — the grant is per-machine. `./uninstall.sh` reverses it (`--purge` also
removes the token).

### Why the transfer method matters

This app is signed but **not notarized**, because notarization needs a Developer
ID certificate and this project signs with an Apple Development one. Gatekeeper
only inspects files carrying a `com.apple.quarantine` flag, which is set by
browsers, AirDrop, and email clients — but not by `scp`/`rsync`/USB. So:

- **scp / rsync / USB** → no quarantine → launches normally.
- **browser / AirDrop** → quarantined → macOS blocks it. The installer detects
  this, explains it, and clears the flag with `xattr -dr com.apple.quarantine`.

`spctl --assess` reports `rejected` for this app either way. That is expected
and does not prevent it from running unquarantined.

To distribute beyond your own machines, you need an Apple Developer Program
membership, a Developer ID Application certificate, and a notarization pass
(`xcrun notarytool submit` + `stapler staple`).

## Browser UI

Open `http://localhost:8765/` (or `http://<mac-lan-ip>:8765/` from another
machine on the LAN). The single-page UI is served straight from the daemon — no
build step, nothing to host separately.

- **Token, entered once.** On first visit it prompts for the bearer token and
  stores it in the browser's `localStorage`, so you don't paste it again. A
  rejected token (`401`) clears the stored value and re-prompts; **Forget token**
  in the header clears it on demand.
- **Capture.** Click **Take screenshot** to grab the display under the cursor.
  A quality slider (1–100) controls JPEG compression. The result renders inline
  with its dimensions, size, and capture time.
- **Full-screen zoom & pan.** Click the screenshot to open it in a
  full-viewport modal. Scroll (or trackpad-pinch) to zoom toward the pointer,
  drag to pan, and use the toolbar (`+` / `−` / fit) or keys (`+`, `-`, `0`,
  `Esc`). It opens fitted to the window and zooms up to 8×.
- **Health dot.** The dot next to the title is green when Screen Recording is
  granted on the host and red when it isn't (or a capture returns `503`).
- **Download.** A download link saves the current JPEG with a timestamped name.

The UI is a dark-theme, dependency-free page embedded in the binary at compile
time (`src/index.html`).

## Usage (HTTP API)

```sh
TOKEN=$(cat ~/.config/screenshotr/token)

# Capture the display under the cursor
curl -H "Authorization: Bearer $TOKEN" \
     "http://localhost:8765/screenshot?quality=85" -o shot.jpg

# Health / diagnostics (no auth)
curl -s http://localhost:8765/healthz
# {"status":"ok","version":"0.1.0","screen_recording":true,"active_display":1}
```

| Endpoint | Auth | Response |
|---|---|---|
| `GET /` | none | `200` `text/html` — the browser UI (token is entered in-page) |
| `GET /screenshot?quality=<1-100>` | Bearer token | `200` `image/jpeg`, native resolution. `quality` defaults to 85. |
| `GET /healthz` | none | `200` JSON: version, permission status, active display ID |

Status codes are distinct so a caller can tell failures apart:

- `401` — missing or bad token
- `503` — Screen Recording permission not granted (JSON body names the fix)
- `500` — capture or encode failure

## Configuration

Set in `packaging/launchagent.plist.in`, then re-run `make install`.

| Variable | Default | Notes |
|---|---|---|
| `SCREENSHOTR_BIND` | `0.0.0.0:8765` | See [Security](#security) |
| `SCREENSHOTR_TOKEN_FILE` | `~/.config/screenshotr/token` | Path only, never the token itself |
| `RUST_LOG` | `info` | |

## Security

**This binds to `0.0.0.0` by default, so any host on your LAN can reach a
screen-capture endpoint.** That is a deliberate choice; the safeguards are:

- A token is mandatory — the service refuses to start without one.
- Tokens are compared in constant time.
- The token lives in a `chmod 600` file, never in the LaunchAgent plist (which
  is world-readable).
- Traffic is plain HTTP, so the token and your screen contents cross the network
  unencrypted. Use it only on a network you trust.

To restrict to this machine, set `SCREENSHOTR_BIND=127.0.0.1:8765`.

## Signing

`SIGN_ID` in the `Makefile` defaults to an Apple Development identity. Override
it for your own:

```sh
make install SIGN_ID="Apple Development: Your Name (TEAMID)"
```

Signing is **not optional**. macOS attributes the Screen Recording grant to a
binary's *designated requirement*. For ad-hoc signed code that requirement is a
literal `cdhash`, which changes on every build — so the grant would reset each
time you rebuild. A certificate-backed signature pins the requirement to
identifier + certificate chain instead, and survives rebuilds:

```
designated => identifier "com.keithsimon.screenshotr" and anchor apple generic
              and certificate leaf[subject.CN] = "Apple Development: ..."
```

`make verify` hard-fails if the requirement ever degrades to a `cdhash`. Never
ad-hoc sign under this bundle ID, even once — it poisons the TCC record for it.

## Make targets

| Target | Purpose |
|---|---|
| `install` | build → sign → verify → install → load |
| `dist` | build a redistributable tarball in `dist/` |
| `status` | agent state and pid |
| `logs` | tail stdout/stderr |
| `unload` / `load` | stop / start the agent |
| `uninstall` | remove app and agent (keeps the token) |

## Notes

- Captures are serialised behind a mutex; concurrent ScreenCaptureKit calls buy
  nothing for a request-driven service.
- `build.rs` adds an rpath to `/usr/lib/swift`. The Swift bridge links
  `libswift_Concurrency.dylib` via `@rpath` but sets no `LC_RPATH`, so without
  it the binary dies at launch.
- Images come back at the display's full backing-store resolution, which on a
  scaled Retina mode is larger than the physical panel (e.g. 4112×2658 for a
  3456×2234 panel). That is the true capture resolution, not an error.
