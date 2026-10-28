# Portal save dialog, no filesystem widening

Status: superseded in part by [0004](0004-appimage-packaging.md) — there is no
Flatpak build any more. The portal decision itself stands and the code still
follows it; the sandbox framing below is the historical context.

The flatpak grants only `--talk-name=org.sequ.timetrack` plus `--filesystem=xdg-run/timetrack`, so its `$HOME` is the sandbox and the "Writes ~/..." label misleads. We save via the `org.freedesktop.portal.FileChooser` (`ashpd` SaveFile) with no manifest widening, because reaching host files through the portal needs no permanent sandbox hole while granting home/documents would.
