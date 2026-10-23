# TODO

## Verify on niri

- [ ] `notulist-server` + `notulist-input`: the pill appears below the text cursor, follows focus between windows, and shows Loading model, Ready, Waiting/Hearing with the waveform, Decoding, Finishing, then the Committed flash.
- [ ] `cargo run --example replay` in `nix develop .#cuda` prints a timeline with corrections, and with `notulist-input` running types the same final text into a focused field.
- [ ] Holding Super+V dictates and releasing stops. Super+T toggles. Super+Shift+V cancels. With no niri binds, check that no `v` or `t` leaks into text fields in your usual apps (GTK, Firefox, terminals, Electron). If one does, bind that chord to a no-op in niri.
- [ ] Hold mode with the dotool fallback: typing waits until Super is released and doesn't stop the session.
- [ ] The first words appear about 1–2 s after you start speaking with large-v3-turbo, and en/nl sessions keep one language.
- [ ] A missing microphone (`[daemon.microphone] device_name = "nope"`) shows Failed, and the next toggle tries again.
- [ ] The mic button toggles dictation, and the field keeps keyboard focus after the click.
- [ ] Password and PIN fields show "Password" and receive no text, through either the input method or dotool.
- [ ] Losing the field mid-session stops dictation.
- [ ] Apps without text-input-v3 fall back to dotool. The modifier guard waits up to 2 s for the Mod+V chord to be released, then stops the session if a modifier is pressed again.
- [x] `cargo run --example popup` shows the indicator on the desktop.
- [ ] `cargo run --example probe` still logs pointer events on the popup.

## Open issues

- [ ] Seat takeover: niri gives the input-method seat to the newest client, so `notulist-input` gets `unavailable` (for example while the gallery or probe runs) and never reclaims it. Re-bind after a backoff, or rely on a systemd `Restart=on-failure` unit.
- [ ] Untested apps: Firefox, GTK, Qt 6, and Electron/Chromium (needs `--enable-wayland-ime --wayland-text-input-version=3`).
- [ ] Untested behaviour: long preedit rendering per app, and apps that ignore `delete_surrounding_text`.
- [ ] Optional D-Bus activation file, so `notulist-server record toggle` can start the server on demand.
- [ ] nixos-config: set `[ime.popup.phase_styles.*]` colours from the base16 palette sottomano uses.
- [ ] Labels look faint at 1x; boost glyph coverage or use gamma-correct blending.
- [ ] Shape-only phase cues, so the label can be turned off without losing information.
- [ ] Rename the remote (wvhulle/voxtype → wvhulle/notulist) and the local directory.
- [ ] Drop the probe example once the indicator is verified.

## Dev notes

- Run tests in an isolated shell: `nix develop -i --keep HOME -c cargo test --workspace`. An inherited `LD_LIBRARY_PATH` (for example from sottomano's dev shell) breaks binaries with `GLIBC_2.43 not found`.
- Run Wayland programs in a plain `nix develop`, or keep `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` when using `-i`.
