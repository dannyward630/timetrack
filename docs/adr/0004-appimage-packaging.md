# AppImage packaging, no Flatpak

The GUI ships as one portable AppImage bundling `timetrack-gui` and `timetrack-service` (built by `build-appimage.sh` via linuxdeploy + appimagetool), because a sandbox forced the service onto the host with a manual D-Bus activation install while an AppImage starts its bundled service itself. The portal save dialog stays: with no sandbox the fallback writes to the real home directory, so its label names `~/` instead of a sandbox.
