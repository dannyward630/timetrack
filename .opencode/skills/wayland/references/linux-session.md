# Linux Wayland session reference

Protocol facts above come from wayland.app; this file is the Linux runtime
around it: how to tell you are on Wayland and how to observe the wire.

## Session check (in order)

1. `echo "$XDG_SESSION_TYPE $WAYLAND_DISPLAY $XDG_RUNTIME_DIR $XDG_CURRENT_DESKTOP"` —
   expect `wayland`, a socket name like `wayland-0`, a writable
   `XDG_RUNTIME_DIR` (usually `/run/user/$UID`), and the desktop.
2. `ls -l "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY"` — the compositor-owned socket
   must exist and be a socket. Missing socket = wrong `XDG_RUNTIME_DIR`, the
   compositor is not running, or you are inside a container/flatpak without
   the socket forwarded.
3. `loginctl show-session "$(loginctl | grep "$(whoami)" | awk '{print $1}' | head -1)" -p Type -p Display`
   — confirms what logind believes.
4. Never hardcode `wayland-0`. Respect `WAYLAND_DISPLAY`; fall back to
   `wayland-0` only when unset.

## Compositor detection

- `pgrep -a -f 'gnome-shell|kwin_wayland|sway|hyprland|weston|mutter|cage|labwc'`
- GNOME/Mutter, KDE/KWin, Sway and other wlroots compositors, Hyprland, and
  Weston all speak the core protocol here, but differ in which extension
  protocols they advertise. When a global is missing, check the compositor
  first: `wayland-info` lists exactly what *this* compositor exposes.
- Nested testing: run a nested compositor (`weston`, `cage`, `sway -c <cfg>`)
  from an X11 or Wayland session to isolate client behavior.

## XWayland interop

- `xprop -root 2>/dev/null | head` succeeding plus `echo "$DISPLAY"` set means
  XWayland is up. Wayland-native clients ignore `DISPLAY`; X11 clients need it.
- Window rules, global coordinates, and screen capture behave differently
  across the boundary. Identify which side the target client is on before
  debugging input or focus.

## Toolkit env vars (client side)

- Qt: `QT_QPA_PLATFORM=wayland` forces Wayland; `=xcb` forces XWayland.
- GTK/GDK: `GDK_BACKEND=wayland` vs `x11`. Current GTK prefers Wayland when
  `WAYLAND_DISPLAY` is set.
- SDL: `SDL_VIDEODRIVER=wayland`.
- Firefox/Chromium/Electron: `MOZ_ENABLE_WAYLAND=1`,
  `--ozone-platform-hint=wayland` (check per-version flags).
- Set these per-command for bisection, not globally, to avoid breaking the
  rest of the session.

## Debug tools

- `WAYLAND_DEBUG=1 ./your-client` — dumps every request/event on stderr.
  `WAYLAND_DEBUG=client` / `=server` filters one side when both are yours.
  Start here for any "wrong sequence" bug.
- `wayland-info` (from `wayland-utils`) — the registry dump: globals,
  interfaces, versions. Compare against what your code expects to bind.
- `wev` — key/pointer/touch event viewer, the Wayland analogue of `xev`.
- `weston-info` — same idea via libwayland-client; useful where
  `wayland-info` is unavailable.
- `wlr-randr` / `kscreen-doctor` / `gnome-randr` — output geometry on
  wlroots / KDE / GNOME when `wl_output` values look suspect.
- `strace -e trace=connect,sendmsg,recvmsg -p <pid>` — last resort to confirm
  which socket a client actually opened.

## Frequent failure modes

- `failed to connect toWayland display` — wrong `XDG_RUNTIME_DIR` permissions
  (must be `0700`, owned by you), stale `WAYLAND_DISPLAY`, or sandbox without
  `--socket=wayland` (flatpak) / `-v $XDG_RUNTIME_DIR/$WAYLAND_DISPLAY:...`
  (containers).
- Black / unresponsive surface — `attach` without `commit`, `damage` missing
  after drawing, or waiting on a `frame` callback that never re-arms. Check
  `WAYLAND_DEBUG` for the `commit` first.
- No input — surface has no keyboard/pointer focus (`wl_pointer.enter` /
  `wl_keyboard.enter` never arrived), wrong `wl_surface.set_input_region`, or a
  popup/grab owned by another client.
- Clipboard/drag silently empty — mismatched MIME types between
  `wl_data_source.offer` and `wl_data_offer.receive`, or `finish` called after
  an `accept(NULL)`. Trace both sides.
