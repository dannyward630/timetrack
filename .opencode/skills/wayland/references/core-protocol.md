# Core Wayland protocol reference

Distilled from https://wayland.app/protocols/wayland
(`wayland.xml`, core). Shorthand: `C` = client request, `S` = server event.
Version gates matter: bind at most the version you implement, check
`wl_registry.global` version before using newer requests.

## Connection and globals

### wl_display v1 — singleton, object 1

- `C sync(callback: new wl_callback)` — barrier. Server emits
  `wl_callback.done` after handling everything before it.
- `C get_registry(registry: new wl_registry)` — create the registry. Call
  sparingly; server-side resources live until disconnect, not until the proxy
  dies.
- `S error(object_id, code, message)` — fatal, non-recoverable. Codes are
  per-interface; the global enum is `invalid_object=0`, `invalid_method=1`,
  `no_memory=2`, `implementation=3`.
- `S delete_id(id)` — server dropped a client-created object; the ID is
  reusable. Internal to object-ID management.

### wl_registry v1 — singleton

- `C bind(name, id: new)` — bind numeric `name` from a `global` event to a
  client object. `new_id` without an interface has special wire-format rules.
- `S global(name, interface, version)` — one per current global, then more on
  hotplug. End of the initial burst is marked by the client's `sync` roundtrip.
- `S global_remove(name)` — global is gone. If bound, keep the object valid,
  ignore further requests to it, destroy it. This races with in-flight client
  requests by design.

### wl_callback v1 — frozen

- `S done(callback_data)` — destructor. Fires for `wl_display.sync` and
  `wl_surface.frame`. Undefined `callback_data` for sync; ignore it.

### wl_fixes v2 — global fix-ups

Corrects defects in other core interfaces without bumping their versions.

- `C destroy`, `C destroy_registry(registry)`, `C ack_global_remove(name)`

## Compositor, surfaces, regions

### wl_compositor v7 — singleton global

- `C create_surface(id: new wl_surface)`
- `C create_region(id: new wl_region)`
- `C release()` — since v7. Destroys only the proxy.

### wl_surface v7

Rectangular area; displayable on zero or more outputs. Double-buffered state:
`attach` + `damage` + `commit` applies atomically.

- `C destroy`, `C attach(buffer, x, y)`, `C damage(x, y, w, h)`,
  `C frame(callback: new wl_callback)`,
  `C set_opaque_region(region)`, `C set_input_region(region)`,
  `C commit()`, `C set_buffer_transform(transform)`,
  `C set_buffer_scale(scale)`, `C damage_buffer(x, y, w, h)`,
  `C offset(x, y)` — since v6, `C get_release(release: new wl_release)` — since v7
- `S enter(output)`, `S leave(output)` — surface entered/left an output.
- `S preferred_buffer_scale`, `S preferred_buffer_transform` — compositor hints.
- Pitfalls: attaching NULL detaches; `commit` on an unattached surface still
  processes frame callbacks. Destroying a `wl_buffer` while attached is legal;
  contents persist until replaced. Reuse a buffer only after `wl_buffer.release`
  or the frame callback of the attaching commit, whichever signals first that
  the compositor holds a copy.

### wl_region v7

- `C destroy`, `C add(x, y, w, h)`, `C subtract(x, y, w, h)` — builds opaque /
  input areas for `wl_surface.set_*_region`.

### wl_subcompositor v1 / wl_subsurface v1

- `wl_subcompositor.C destroy`, `C get_subsurface(id, surface, parent)`
- `wl_subsurface.C destroy`, `C set_position(x, y)`, `C place_above(sibling)`,
  `C place_below(sibling)`, `C set_sync()`, `C set_desync()`
- Sub-surface stacking and sync mode (synchronized to parent commit vs
  independent). Default is synchronized.

## Buffers: wl_shm, wl_shm_pool, wl_buffer

### wl_shm v3 — singleton global

- `C create_pool(id: new wl_shm_pool, fd, size)` — mmap's `size` bytes of `fd`.
- `C release()` — since v2. Created pools survive release.
- `S format(format)` — one per supported pixel format. `argb8888`/`xrgb8888`
  are the only formats every renderer must support; the enum also covers YUV,
  10-bit, float, and indexed formats. Expect formats newer than your headers.
- Errors since v3: `invalid_stride=1`, `invalid_fd=2`
  (`invalid_format=0` deprecated).

### wl_shm_pool v3

- `C create_buffer(id, offset, width, height, stride, format)` — buffer keeps a
  reference to the pool; destroying the pool right after is valid.
- `C destroy()` — mmapped memory goes away with the last buffer.
- `C resize(size)` — grow only; client must first grow the backing file.
- Errors since v3: `invalid_format=0`, `invalid_stride=1`.

### wl_buffer v1 — frozen

- `C destroy()`
- `S release()` — compositor no longer uses the buffer; safe to reuse backing
  storage. Channels are electrical (transfer-function encoded); alpha, when
  present, is premultiplied unless another extension says otherwise.

## Seats and input

### wl_seat v11

Group of keyboards, pointer, touch devices. One `wl_data_device` per seat.

- `C get_pointer(id)`, `C get_keyboard(id)`, `C get_touch(id)`, `C release()` —
  since v8.
- `S capabilities(capabilities: pointer/keyboard/touch)`, `S name(name)` —
  since v2. Re-emitted when devices appear/disappear.

### wl_pointer v11

- `C set_cursor(serial, surface, hotspot_x, hotspot_y)`, `C release()` — since v3.
- `S enter(serial, surface, surface_x, surface_y)`, `S leave(serial, surface)`,
  `S motion(time, surface_x, surface_y)`, `S button(serial, time, button, state)`,
  `S axis(time, axis, value)`, `S frame()`,
  `S axis_source(source)` — since v5, `S axis_stop(time, axis)` — since v5,
  `S axis_discrete(axis, discrete)` — since v5,
  `S axis_value120(axis, value120)` — since v8,
  `S axis_relative_direction(axis, direction)` — since v8,
  `S warp(time, x, y)` — since v11.
- Events between two `frame` events belong to one logical input group.

### wl_keyboard v11

- `C release()` — since v3.
- `S keymap(format, fd, size)`, `S enter(serial, surface, keys)`,
  `S leave(serial, surface)`, `S key(serial, time, key, state)`,
  `S modifiers(serial, depressed, latched, locked, group)`,
  `S repeat_info(rate, delay)` — since v4.

### wl_touch v11

- `C release()` — since v3.
- `S down(serial, time, surface, id, x, y)`, `S up(serial, time, id)`,
  `S motion(time, id, x, y)`, `S frame()`, `S cancel()`,
  `S shape(id, major, minor)` — since v6,
  `S orientation(id, orientation)` — since v6.

### wl_output v4

Part of compositor geometry.

- `C release()` — since v3.
- `S geometry(x, y, physical_width/height, subpixel, make, model, transform)`,
  `S mode(flags, width, height, refresh)`, `S done()` — groups one
  description burst, `S scale(factor)`, `S name(name)` — since v2,
  `S description(description)` — since v2.

## Clipboard and drag-and-drop

### wl_data_device_manager v4 — singleton global

- `C create_data_source(id: new wl_data_source)`,
  `C get_data_device(id: new wl_data_device, seat)`, `C release()`
- `dnd_action` enum used by offer/source negotiation: `none`, `copy`, `move`, `ask`.

### wl_data_source v4 — source side

- `C offer(mime_type)` — repeat per type. `C destroy()`.
  `C set_actions(dnd_actions)` — since v3, drag-and-drop only, once, before
  `start_drag`.
- `S target(mime_type)` — destination accepted type (NULL = none).
  `S send(mime_type, fd)` — write data then close fd.
  `S cancelled()`, `S dnd_drop_performed()` — since v3,
  `S dnd_finished()` — since v3, `S action(action)` — since v3.

### wl_data_offer v4 — destination side

- `C accept(serial, mime_type)` — NULL = reject. Since v3 this decides the
  drag-and-drop outcome.
- `C receive(mime_type, fd)` — may be called multiple times, before or after
  drop. Read to EOF then close.
- `C destroy()`, `C finish()` — since v3 (drop succeeded; afterwards only
  `destroy` is legal), `C set_actions(dnd_actions, preferred_action)` — since v3.
- `S offer(mime_type)` — one per type at creation.
  `S source_actions(...)` — since v3, `S action(...)` — since v3, most recent
  wins; honor the last one at `drop` time.
- Errors: `invalid_finish=0`, `invalid_action_mask=1`, `invalid_action=2`,
  `invalid_offer=3`.

### wl_data_device v4 — one per seat

- `C start_drag(source, origin, icon, serial)`, `C set_selection(source, serial)`,
  `C release()` — since v2.
- `S data_offer(id: new wl_data_offer)`, `S enter(serial, surface, x, y, id)`,
  `S leave()`, `S motion(time, x, y)`, `S drop()`, `S selection(id)`.

## Deprecated shell (do not use for new code)

### wl_shell v1 / wl_shell_surface v1

Superseded by `xdg-shell`. Retained so old clients keep running.

- `wl_shell.C get_shell_surface(id, surface)`
- `wl_shell_surface.C pong(serial)`, `C move(seat, serial)`,
  `C resize(seat, serial, edges)`, `C set_toplevel()`, `C set_transient(parent, x, y, flags)`,
  `C set_fullscreen(method, framerate, output)`, `C set_popup(seat, serial, parent, x, y, flags)`,
  `C set_maximized(output)`, `C set_title(title)`, `C set_class(class_)`
- `wl_shell_surface.S ping(serial)`, `S configure(edges, width, height)`,
  `S popup_done()`

Full signatures, enum values, and per-version notes: the wayland.app page for
each interface.
