# TODO

## Verify on niri

- [ ] `notulist daemon` + `notulist-ime`: the pill appears below the text cursor, follows focus between windows, and shows Loading model, Ready, Waiting/Hearing with the waveform, Decoding, Finishing, then the Committed flash.
- [ ] `cargo run --example replay` in `nix develop .#cuda` prints a timeline with corrections, and with `notulist-ime` running types the same final text into a focused field.
- [ ] A missing microphone (`[daemon.microphone] device_name = "nope"`) shows Failed, and the next toggle tries again.
- [ ] The mic button toggles dictation, and the field keeps keyboard focus after the click.
- [ ] Password and PIN fields show "Password" and receive no text, through either the input method or dotool.
- [ ] Losing the field mid-session stops dictation.
- [ ] Apps without text-input-v3 fall back to dotool. The modifier guard waits up to 2 s for the Mod+V chord to be released, then stops the session if a modifier is pressed again.
- [x] `cargo run --example popup` shows the indicator on the desktop.
- [ ] `cargo run --example probe` still logs pointer events on the popup.

## Open issues

- [ ] Seat takeover: niri gives the input-method seat to the newest client, so `notulist-ime` gets `unavailable` (for example while the gallery or probe runs) and never reclaims it. Re-bind after a backoff, or rely on a systemd `Restart=on-failure` unit.
- [ ] Untested apps: Firefox, GTK, Qt 6, and Electron/Chromium (needs `--enable-wayland-ime --wayland-text-input-version=3`).
- [ ] Untested behaviour: long preedit rendering per app, and apps that ignore `delete_surrounding_text`.
- [ ] Packaging: systemd user units for `notulist daemon` and `notulist-ime`, an optional D-Bus activation file, and a home-manager module. The voxtype modules are gone.
- [ ] nixos-config: switch the input from voxtype to this flake (`packages.cuda`), write `~/.config/notulist/config.toml` with the `[daemon.*]` keys (the voxtype keys were renamed, see README), and set `[ime.popup.phase_styles.*]` colours from the base16 palette sottomano uses.
- [ ] The mic button icon reads like a thermometer at 14 px; draw a clearer microphone glyph.
- [ ] Shape-only phase cues, so the label can be turned off without losing information.
- [ ] Rename the remote (wvhulle/voxtype → wvhulle/notulist) and the local directory.
- [ ] Drop the probe example once the indicator is verified.

## Dev notes

- Run tests in an isolated shell: `nix develop -i --keep HOME -c cargo test --workspace`. An inherited `LD_LIBRARY_PATH` (for example from sottomano's dev shell) breaks binaries with `GLIBC_2.43 not found`.
- Run Wayland programs in a plain `nix develop`, or keep `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` when using `-i`.
