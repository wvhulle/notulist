# Fork TODO: input-method dictation on niri

This file tracks the `wvhulle/dictation` stack. Upstream's own notes stay in `TODO.md`.

## Stack (on top of `dev@upstream`)

1. `fix(streaming): stop the session when a modifier is pressed while typing`
2. `feat(output): input-method-v2 driver behind the input-method feature`
3. `chore(nix): build the fork with the input-method feature and wayland` (`wvhulle/dictation` bookmark)
4. `test(examples): probe pointer delivery to input-method popups on the compositor` (working copy)

To rebase onto a new upstream: `jj git fetch --remote upstream && jj rebase -s <stack-root> -d dev@upstream`.

## Done

- [x] Modifier guard for virtual-keyboard streaming. It waits up to 2 s for the Mod+V chord to be released, then stops the session if a modifier is pressed again.
- [x] input-method-v2 driver:
  - preedit for the last `REVISION_LAG_WORDS + 1` words, commits for older ones, and `delete_surrounding_text` for corrections that reach into committed text
  - fields are numbered on each `activate`, and losing the field stops the session
  - apps without text-input-v3 fall back to dotool
- [x] Fork logic kept out of upstream hot spots:
  - `daemon.rs` has about 33 changed lines and `streaming.rs` about 17
  - the rest lives in `output/input_method/`, `output/held_modifier_safety.rs` and `output/streaming/`
- [x] Examples, run with the cargo aliases inside `nix develop`:
  - `cargo input-method-replay` replays a scripted dictation into the focused field
  - `cargo input-method-probe` shows a popup at the text cursor and logs pointer events
- [x] Probe result on niri:
  - the popup appears below the text cursor and follows focus between windows
  - clicks reach the popup, and the field keeps keyboard focus

## Next: dictation indicator at the text cursor

The pill shows on every focused text field, with a live waveform plus a label, and has a clickable mic button.

- [ ] `src/output/input_method/dictation_indicator_state.rs` (pure, tested): map the daemon state onto what the pill shows.

  | Daemon state | Pill | Colour |
  |---|---|---|
  | `idle` | On | dim |
  | `recording` / `streaming` | waveform + Listening | accent |
  | `transcribing` | spinner + Processing | warning |
  | password or PIN field | Not in password fields | dim |

- [ ] `dictation_indicator_painter.rs`:
  - draw with `tiny-skia` 0.11 (shapes) and `ab_glyph` 0.2 (label)
  - resolve the font once with `fc-match -f '%{file}' sans`
  - add a pure layout function returning pill size and mic-button rect, with unit tests
- [ ] SHM double buffer:
  - two slots released on `wl_buffer.release`
  - swizzle tiny-skia's RGBA into `Argb8888`
- [ ] Extend `wayland_input_method_connection.rs`:
  - bind `wl_compositor` v6 (for `preferred_buffer_scale`), `wl_shm` and the seat pointer
  - create the popup on field activation and destroy it on deactivation
  - redraw on a calloop timer only while state is animated
- [ ] State feed: one line in `daemon.rs` `update_state` calls `output::input_method::publish_daemon_state(state_name)`, which does nothing when there's no connection.
- [ ] Audio levels: `tokio::spawn(osd::ipc::run_ipc_loop(...))` from `DictationFieldIntegration::new`, forwarding `AudioFrame`s to the connection thread.
- [ ] Mic button: a click inside its rect sends `kill(getpid(), SIGUSR1)` when idle or `SIGUSR2` when listening, the same signals `voxtype record toggle` uses. This needs no daemon changes.
- [ ] Password fields:
  - handle `ContentType` (purpose password or PIN, hint `sensitive_data` or `hidden_text`)
  - `InputMethodFieldWriter::for_focused_field` refuses such fields
  - `held_modifier_safety` also stops dotool sessions there before anything is typed
- [ ] Config: new `src/config/input_method_indicator.rs` for `[input_method_indicator]`:
  - keys `enabled`, `reduced_motion`, `font`, `background`, `foreground`, `accent` and `warning`
  - a single field on the root `Config`
  - `input_method_indicator.` added to `NOT_SETTABLE_TABLES` in `config/schema.rs` tests, so it needs no schema keys
- [ ] Example `cargo input-method-indicator-gallery`: cycles On, Listening (synthetic levels), Processing and the password-field state, without dictating.
- [ ] nixos-config: set the indicator colours from the base16 palette sottomano uses.

## Open issues

- [ ] Seat takeover: niri gives the input-method seat to the newest client, so the daemon gets `unavailable` (for example when a probe runs) and never reclaims it. Decide between re-binding after a delay and documenting it.
- [ ] Untested apps: Firefox, GTK, Qt 6, and the Electron/Chromium fallback (needs `--enable-wayland-ime --wayland-text-input-version=3` for input-method use).
- [ ] Untested behaviour: long preedit rendering per app, and apps that ignore `delete_surrounding_text`.
- [ ] Flaky upstream test under parallel load: `setup::binary::tests::verify_binary_runs_accepts_a_working_binary`.
- [ ] Decide what to push to `origin` (wvhulle/voxtype) and whether to open an upstream PR for commit 1.
- [ ] Drop or keep the probe commit once the indicator works.

## Dev notes

- Run tests in an isolated shell: `nix develop -i --keep HOME -c cargo test`. An inherited `LD_LIBRARY_PATH` (for example from sottomano's dev shell) breaks binaries with `GLIBC_2.43 not found`.
- Run Wayland programs in a plain `nix develop`, or keep `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR` when using `-i`.
- nixos-config builds the fork from `git+file:…?ref=refs/heads/wvhulle/dictation`. Run any jj command in the fork, then `nix flake update voxtype`, before rebuilding.
