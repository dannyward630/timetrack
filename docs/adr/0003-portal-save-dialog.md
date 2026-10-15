# Portal save dialog, no filesystem widening

The flatpak grants only `--talk-name=org.sequ.timetrack` plus `--filesystem=xdg-run/timetrack`, so its `$HOME` is the sandbox and the "Writes ~/..." label misleads. We save via the `org.freedesktop.portal.FileChooser` (`ashpd` SaveFile) with no manifest widening, because reaching host files through the portal needs no permanent sandbox hole while granting home/documents would.
