# AppImage self-update, plus zsync metadata for AppImageUpdate

The AppImage updates itself: on startup the GUI checks the latest GitHub
release once, on a background thread, and only when running as an AppImage
(`APPIMAGE` set, pointing at an existing file). A newer tag with an asset
for this architecture puts `U update to vX.Y.Z` in the footer hints and a
one-line status. The first `U` only arms the install; the second one
downloads the asset and renames it over the running image, then asks for a
relaunch. Nothing installs silently and nothing restarts itself.

Silent auto-install was rejected: replacing the running binary is a
destructive action, and this app confirms those twice (merge, delete). A
failed or offline check is silence, never an error -- an offline launch is
not an update failure. `TIMETRACK_NO_UPDATE=1` skips the check (metered
connections; the GUI smoke rig sets it so pixel assertions never depend on
the network or on whatever release is latest).

The check reads `releases/latest` and compares the tag against the
workspace version, so a tag `vX.Y.Z` and a Cargo bump ship together. The
download must start with the ELF magic, or it is refused: a GitHub error
page served as `200 OK` must never become the user's binary. The temp file
is a sibling `.new` file renamed over the image, so a half-written download
leaves the current image untouched.

The network call is blocking `ureq` with rustls on a dedicated thread --
never from a gpui future, and never tokio (see the workspace manifest for
why that is load-bearing absent). Only the AppImage updates itself: a host
`cargo build` has no release channel, and the CLI ships in that same image,
so updating the image updates both.

Separately, `build-appimage.sh` embeds
`gh-releases-zsync|sequ-r|timetrack|latest|TimeTrack-*<arch>.AppImage.zsync`
via appimagetool `-u`, and `release.yml` publishes the `.zsync` file next
to the image (installing `zsync`, verifying both, checksumming both). That
is for the external AppImageUpdate tool, which does delta downloads; the
in-app updater downloads the whole ~10 MB image and needs neither.

Two packaging traps, for whoever touches this next: appimagetool's
`--appimage-extract-and-run` re-executes from an extracted copy that does
not keep the script's working directory, so the AppDir and the destination
must be absolute paths (a relative `AppDir` fails with "no such file or
directory"); and the `.zsync` lands next to wherever that copy thinks it
runs (in practice `$HOME`), not next to the image, so the script moves it
into place and the release job asserts it is there.
