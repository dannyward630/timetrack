# TimeTrack

[![ci](https://github.com/sequ-r/timetrack/actions/workflows/ci.yml/badge.svg)](https://github.com/sequ-r/timetrack/actions/workflows/ci.yml)

A time tracker for GNOME. You **enter** time — a quick tap for the common case,
or an explicit interval when you are reconstructing the day — and the app tells
you where a week went.

There is no running timer. Every entry is a closed interval you either type in
or add with one of four buttons, and everything is summed: overlapping entries
count twice, because "how much time did I spend" is not "how much of my day did
it occupy".

The data lives in a **service**, not in either front end, so closing the GUI or
the terminal never loses anything, and both can be open at once.

## Architecture

```
  timetrack-gui (AppImage)          timetrack (CLI, static binary)      timetrack-tray (host only)
  gpui 0.2.2, 3 tabs               ratatui TUI + one-shot commands     SNI item: week-total tooltip,
                                                                            per-project quick-add/undo/delete
        |                                  |                                   |
        +--------- D-Bus: org.sequ.timetrack.Entries ----+
                           |
              timetrack-service  (D-Bus activated)
              owns the store; does all aggregation
                           |
                  ~/.local/share/timetrack/store.json
```

| Crate | Role |
|---|---|
| `timetrack-core` | Model, rules, aggregation, atomic JSON storage. No GUI, IPC, async, **or clock**. |
| `timetrack-proto` | The D-Bus contract and a client. All front ends depend on this. |
| `timetrack-service` | D-Bus server. Owns the store, resolves the clock and timezone, persists on every change. |
| `timetrack-cli` | Terminal client: a ratatui TUI plus one-shot subcommands. |
| `timetrack-gui` | Desktop client, built on gpui (upstream, from crates.io). Ships as the AppImage. |
| `timetrack-tray` | Host-only tray item (StatusNotifierItem): service presence, week total, per-project menu. |

The core takes an explicit timestamp and never reads the clock, so the rules are
identical everywhere and testable without any I/O. Aggregation is implemented in
the core but *applied* by the service, so the GUI and the CLI cannot disagree
about where a week starts.

## What works today

| | |
|---|---|
| Four ways to add time | explicit interval, duration ending now, duration ending earlier, quick-add (5/15/30/60) |
| Three ways to take it back | `shorten` moves an endpoint, `undo` removes a quick-add, `delete` is explicit |
| Split and merge | in the GUI (`s` split prompt, `m` merge confirm stating the shrink), the CLI, the core and the service |
| Totals | this ISO week, this month, all time, per project; the service aggregates so both UIs agree |
| Overlap | counted twice, by design; a day over 24h is flagged, never rejected |
| Projects | create, rename, archive and unarchive; archiving keeps the history in totals |
| Persistence | atomic writes; survives restart; corrupt or newer stores are refused, never silently reset |
| CSV export | current or chosen week/month (`--at YYYY-MM-DD`, default now) or all time; the CLI prints or writes a file, the GUI Export tab saves through the system dialog with period-stamped names |
| System tray | week-total tooltip with over-24h attention state, per-project quick-add / undo / delete-confirm, Open TimeTrack, Quit (host only) |

The GUI Export tab picks the period with a previous/next stepper (labelled
`2026-W41` / `2026-09`) plus a typed `YYYY-MM-DD` day, shows a preview of
exactly what the file will hold (scoped rows, oldest first), and saves through
the system save dialog — `timetrack-export-week-2026-W41.csv` and siblings by
default, so one week never overwrites the last. Cancelling the dialog does
nothing; without a portal it falls back to saving directly in your home
directory.

## Building

Requires Rust 1.85 or newer (the crates are edition 2024).

```sh
cargo build --release
# The tray's round-trip test spawns the debug service binary, which `cargo test`
# does not build (it compiles test harnesses, not binaries):
cargo build --package timetrack-service
cargo test --workspace --features timetrack-gui/portal-stub   # unit tests (GUI tests need the system libs below)
bash scripts/e2e.sh           # end-to-end checks against a real service
bash scripts/gui-smoke.sh     # headless GUI checks: paint, quick-add click, keyboard, confirm dialogs, tabs, export picker/preview/save
```

The `portal-stub` feature compiles in the save-dialog stub the GUI smoke
check drives: there is no live portal on a headless X server. A released
binary is built **without** it, so there is no stub in it to arm — the
system save dialog cannot be talked out of the way by an environment
variable.

CI runs those same commands on every pull request and on `main`
(`.github/workflows/ci.yml`: rustfmt, clippy, `cargo test --workspace`, then the
service rig and the GUI smoke check, each job on its own so a red run names the
one that failed). A failed smoke check uploads the screenshots and logs it took
(`TT_SMOKE_ARTIFACTS`) as a run artifact. Pushing a `v*` tag runs `release.yml`,
which calls that workflow first and builds the CLI tarballs and the AppImage
only afterwards; dispatching `release.yml` by hand runs the same build and
verification with `dry_run` on by default, so the packaging path can be checked
without cutting a release.

On Linux the GUI additionally needs the system libraries gpui links against:

```sh
# Debian/Ubuntu
apt install libxkbcommon-dev libxkbcommon-x11-dev libfreetype-dev libfontconfig-dev
```

`gui-smoke.sh` runs the GUI on Xvfb, screenshots it and drives it through the
X11 XTest extension, so it needs no display of its own — but Xvfb implements
no DRI3 and Mesa's GPU drivers need it to present, so the GUI needs a software
Vulkan driver there (Arch: `vulkan-swrast`, Debian/Ubuntu:
`mesa-vulkan-drivers`; the script also accepts an extracted one via
`TT_LAVAPIPE_DIR`). Without it the window opens and stays black, which the
script reports as a driver problem rather than as a failure of the app.

The GUI's keyboard was the one thing headless checks could not reach: an Xvfb
server has no keymap, so every keysym lookup returns 0 and no keystroke can be
delivered. `setxkbmap` gives it one, so `gui-smoke.sh` now presses `1` and `q`
and asserts both. The window's root element still needs its id and `FocusHandle`
for `on_key_down` to fire at all — that requirement did not go away with the
toolkit.

## Running

Start the service once, then any client:

```sh
timetrack-service &
timetrack                      # TUI
timetrack-gui                  # desktop GUI (host build; or the AppImage below)
timetrack-tray &               # system tray (host only)
timetrack quick -m 15          # or a one-shot command
timetrack status
```

The TUI: `1`-`4` quick-add, `a` 30 minutes, `u` undo, `d` delete, `j`/`k` move,
`h`/`l` change project, `q` quit. Projects are created from the command line
(`timetrack project "Work"`), since the TUI has no text field yet.

The tray lists one submenu per unarchived project with its week total:
`+5m`…`+60m` quick-add rows, an `Undo quick-add` row (enabled only while this
session's last action on that project was a quick-add), and a
`Delete last entry (…)` confirm submenu stating duration and project. Its icon
shows the live week total as a tooltip and flattens with a start-service hint
when the service is away, or asks for attention on version skew and over-24h
days. `Open TimeTrack` launches the GUI through the desktop file's `Exec`;
`Quit` stops the tray only, never the service.

The tray needs a StatusNotifier watcher on the session bus: KDE, XFCE and
MATE provide one; on GNOME install a tray extension first, or the icon has
nowhere to appear. It runs on the host (never sandboxed), next to the
service.

### The tray has to be installed once

Same pattern as the service: a binary in `~/.local/bin`, the desktop file
in `~/.local/share/applications`, and a copy of it in `~/.config/autostart`
so it starts at login:

```sh
install -Dm755 target/release/timetrack-tray ~/.local/bin/timetrack-tray
install -Dm644 data/org.sequ.timetrack.tray.desktop ~/.local/share/applications/org.sequ.timetrack.tray.desktop
install -Dm644 data/org.sequ.timetrack.tray.desktop ~/.config/autostart/org.sequ.timetrack.tray.desktop
```

D-Bus activation is unchanged by all of this: the tray connects anonymously
and never owns `org.sequ.timetrack`, so it can neither steal the service's
name nor keep a stale service alive — it only reads. The AppImage ships no
tray in v1 (it bundles only the GUI and the service); the tray is
host-only until it is decided whether it belongs in the image.

### Tray on GNOME, and why the AppImage ships none

GNOME Shell provides no StatusNotifier watcher, so the tray icon has nowhere
to appear there unless a tray extension (e.g. an AppIndicator extension)
registers one. KDE, XFCE and MATE ship a watcher, and the icon shows as-is.

The tray is host-only in v1, deliberately: it runs on the host next to the
service and is installed with the autostart file below. Until it is decided
whether the tray belongs inside the AppImage, run it from a host build.

### The service has to be installed once (host builds)

The AppImage needs none of this: it bundles the service and starts it in
the background when the app launches, so the session bus always finds one.
The steps below are only for running the GUI from a host `cargo build`,
where no bundle exists to start the service. Install the service and a
D-Bus activation file on the host; after that the session bus starts it on
demand:

```sh
install -Dm755 target/release/timetrack-service ~/.local/bin/timetrack-service
mkdir -p ~/.local/share/dbus-1/services
sed 's|/usr/bin/timetrack-service|'"$HOME"'/.local/bin/timetrack-service|' \
  data/org.sequ.timetrack.service.in \
  > ~/.local/share/dbus-1/services/org.sequ.timetrack.service
```

**After upgrading, reinstall the service.** The interface was renamed from
`org.sequ.timetrack.Timer` to `org.sequ.timetrack.Entries` when the timer was
cut. A stale service binary keeps answering to the old name and the GUI reports
`UnknownInterface`. It now says so explicitly rather than claiming the service is
merely absent, because the two need different fixes:

> The GUI and the service are different versions. Reinstall whichever one is
> older so both speak `org.sequ.timetrack.Entries`.

To run it at login instead:

```sh
mkdir -p ~/.config/systemd/user
cp data/org.sequ.timetrack.service ~/.config/systemd/user/
systemctl --user enable --now org.sequ.timetrack.service
```

Note that `Exec=` in a D-Bus service file is **not** run through a shell, so it
must be an absolute path to a real executable -- `sh -c ...` does not work.

## The terminal client

`timetrack` is a separate download from the AppImage — it is an ordinary binary:

```sh
cargo install --git https://github.com/sequ-r/timetrack.git timetrack-cli
```

Release tags also publish statically linked musl builds for x86_64 and
aarch64, so a single artifact runs on any Linux regardless of glibc version:

```sh
curl -LO https://github.com/sequ-r/timetrack/releases/latest/download/timetrack-x86_64.tar.gz
tar xzf timetrack-x86_64.tar.gz && install -m755 timetrack-x86_64 ~/.local/bin/timetrack
```

### Commands

The four entry methods are four subcommands, and the three ways to take time
back are three *separate* commands, because they are three different operations:

```sh
timetrack add -p Work -S -90 -E -30 -d "review"   # method 1: explicit interval
timetrack duration -p Work -f 90m                 # method 2: ends now
timetrack past -p Work -f 90m -E -120             # method 3: ended earlier
timetrack quick -p Work -m 15                     # method 4: 5/15/30/60

timetrack shorten e4 -E -45                       # move an endpoint, keep the entry
timetrack undo e7                                 # only removes a quick-add
timetrack delete e4                               # explicit

timetrack export --scope week                     # this ISO week to stdout
timetrack export --scope week --at 2026-10-05 --out oct2.csv  # a chosen week to a file
timetrack export --scope month --at 2026-09-15    # a chosen month to stdout
timetrack export --scope all                      # everything (`--at` ignored)
```

Timestamps accept `-90` for "90 minutes ago" or `09:30` for that time today.
Durations accept `90m`, `1h30m` or `45s`; a bare number means minutes.

The asymmetry is deliberate: `shorten` can never delete anything, and `undo`
**refuses** an entry it did not create, so it cannot lose hand-entered time by
accident.

Note for packagers: **do not set `CARGO_TARGET_*_MUSL_LINKER=musl-gcc`**. Some
distributions' `musl-gcc` (GCC 16.x on Arch as of 2026-09) produces binaries
that segfault at startup on anything that hashes — `std`'s `HashMap`, which
zbus' D-Bus address parser needs. Rust's default linker for musl targets is
correct; the override is the bug. See `.cargo/config.toml`.

## AppImage

The GUI ships as a single portable file bundling `timetrack-gui` and
`timetrack-service`, built with linuxdeploy + appimagetool (fetched on
demand). It needs no install and no service setup: its AppRun starts the
bundled service in the background, then the GUI. If a service is already
running (a host install, or a second copy of the AppImage), the extra copy
fails to own the bus name and exits, and the GUI talks to whichever holds
it.

```sh
./build-appimage.sh            # produces TimeTrack-<arch>.AppImage
chmod +x TimeTrack-x86_64.AppImage
./TimeTrack-x86_64.AppImage
```

Or by hand:

```sh
cargo build --release --package timetrack-gui --package timetrack-service
# ... see build-appimage.sh for the AppDir layout and tool invocations
```

Release tags publish the AppImage alongside the static CLI tarballs, so one
download covers the desktop app on any Linux. The GUI's build needs the
same system libraries as above (`libxkbcommon-dev`,
`libxkbcommon-x11-dev`, `libfreetype-dev`, `libfontconfig-dev` on
Debian/Ubuntu); linuxdeploy bundles the linked ones into the image, so the
runtime needs nothing installed.

### Updates

The AppImage checks for updates itself, once per launch: it reads the
latest GitHub release and compares the tag against its own version. When a
newer release carries an image for your architecture, the footer says so
and offers `U`:

- first `U` arms the install (`press U again to download …`),
- second `U` downloads the new image and replaces the running file,
- relaunch to run it.

Nothing installs without those two presses, and an offline launch stays
silent rather than erroring. `TIMETRACK_NO_UPDATE=1` skips the check
entirely. Host builds (`cargo run`, installed binaries) never check —
there is no release to update to — and `U` there says so.

The image also carries embedded update information
(`gh-releases-zsync|…|TimeTrack-*.AppImage.zsync`), and releases publish
the `.zsync` file next to it, so the external
[AppImageUpdate](https://github.com/AppImage/AppImageUpdate) tool can
delta-update the same file. The in-app updater downloads the whole image
instead (~10 MB) and needs neither.

## Known issues

The things below are open, known, and not worked around. Everything else in
this README works today.

**The tray icon does not appear on GNOME.** GNOME Shell ships no
StatusNotifierItem watcher, so the icon has nowhere to go. Install a tray
extension (an AppIndicator one is the usual choice) or use the desktop
client. See "Tray on GNOME" above.

**The tray is not in the AppImage.** It is host-only for now: the image
bundles the GUI and the service, and the tray is installed separately with
its autostart file. See "The tray has to be installed once" above.

**A corrupt or newer store is refused, not repaired.** The service will
not start rather than start empty, because starting empty would look like
your tracked time had vanished. The file is at
`~/.local/share/timetrack/store.json`; move it aside to start fresh. A
file written by a newer TimeTrack is refused the same way, and says so.

**A store from the stopwatch era cannot be migrated.** That format has no
projects, so there is nothing to migrate it onto. The service says so
plainly and refuses it; move the file aside to start fresh.

**A project cannot be deleted from the GUI.** The GUI can archive and
unarchive, which keeps the history in the totals, but not delete. The
terminal can: `timetrack delete-project <name>`. A project that still has
entries is refused — move or delete the entries first.

**The TUI cannot create a project.** It has no text field for one; create
it from the command line (`timetrack project "Work"`) and it appears in
the TUI at the next refresh.

**The GUI's export preview shows the first rows, not the whole file.** It
shows up to twenty rows, oldest first, in exactly the order the file
carries them — enough to check the scope, not enough to read the file.

## Licence

GPL-3.0-or-later. See `COPYING`.
