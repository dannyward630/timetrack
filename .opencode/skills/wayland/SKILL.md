---
name: Wayland on Linux
description: Work with Wayland clients, compositors, and the core protocol on Linux. Use when debugging a Wayland session, connecting to wl_display, binding globals via wl_registry, creating wl_surface or wl_buffer objects, handling wl_seat input, or looking up any core wayland.xml interface.
---

# Wayland on Linux

Source of truth for the core protocol: https://wayland.app/protocols/wayland
(core `wayland.xml` from https://gitlab.freedesktop.org/wayland/wayland).
Extension protocols (xdg-shell, presentation-time, etc.) live alongside it
on wayland.app but are out of scope here unless the task needs them.

## Workflow

1. **Pin the session.** Read `references/linux-session.md`. Confirm
   `XDG_SESSION_TYPE`, `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`, and which
   compositor owns the session before touching code.
2. **Connect then enumerate.** `wl_display` connect -> `get_registry` ->
   handle `wl_registry.global` / `global_remove` -> `wl_display.sync` as a
   barrier for the initial burst. Bind only what the task needs.
3. **Follow object lifecycles.** Surfaces, buffers, seats, and outputs each
   have attach/commit/release rules. Look up the exact request/event list in
   `references/core-protocol.md` before writing protocol code.
4. **Debug with the wire first.** Set `WAYLAND_DEBUG=1`, reproduce with
   `wayland-info` / `wev`, compare observed events against the reference.
   Fix the sequence, not the symptom.

## Rules

- Core `wl_shell` / `wl_shell_surface` are deprecated. New toplevels use
  `xdg-shell` (stable protocol on wayland.app), not this skill's core set.
- Requests are handled in order, events arrive in order. Use `wl_display.sync`
  + `wl_callback.done` as the barrier; never sleep-and-hope.
- A bound global can disappear at any time (`global_remove`). Keep the object
  valid, stop sending requests, destroy it.
- `wl_buffer` is factory-neutral (`wl_shm`, linux-dmabuf, etc.). The release
  pattern in the reference applies regardless of factory.
- Prefer shared reference over memory: when unsure about a request, event, or
  version gate, read `references/core-protocol.md` or the wayland.app page
  instead of guessing.

## Reference index

- `references/core-protocol.md` — the 23 core interfaces, what each
  request/event does, version notes, object-lifecycle pitfalls.
- `references/linux-session.md` — session env vars, compositor detection,
  XWayland interop, toolkit env vars, debug tools.
