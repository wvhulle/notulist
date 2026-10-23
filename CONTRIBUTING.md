# Contributing

## Layout

- `notulist-client/` defines the D-Bus contract. Every type that crosses the bus lives here, along with the `#[zbus::proxy]`. The daemon implements the same interface in `notulist-server/src/dbus_dictation_service.rs`. Change both sides in the same commit and keep the round-trip tests in `dictation_types.rs` passing.
- `notulist-server/` is a library plus the `notulist-server` binary.
  - `dictation_daemon.rs` binds the bus name, loads whisper and runs the controller. `main.rs` and `examples/replay.rs` both use it, and differ only in their `AudioSource`.
  - `DictationSessionController` is the only owner of the dictation phase.
  - `dictation_session_pipeline.rs` runs one session. It turns `CapturedAudio` into level batches, sliding-window passes and transcript edits, which it emits through `DictationSignalEmitter`. It reports only `SessionMilestone`s back to the controller.
  - `audio::start_capture` is the one place that chooses between the microphone and WAV playback.
  - `keyboard_shortcut.rs` is a second command source next to D-Bus. It feeds the same `DictationCommand` channel, so the controller stays the only owner of the phase. `ShortcutTracker` is pure; test chord behaviour there.
  - `WhisperModelContext` owns one whisper state for the model's lifetime, warmed up at load. Don't create a state per pass: each one builds a GPU backend.
  - The sliding window decides the session language once (`language_detection_audio_secs`) and passes it to every later pass.
- `notulist-input/`:
  - `input_method/wayland_input_method_connection.rs` runs a calloop thread. It applies field batches and draws the latest `IndicatorFrame`; it knows nothing about phases or edits.
  - `dictation_bus_client.rs` runs on tokio. It feeds bus signals into `IndicatorPhaseTracker` and `LevelBars` and sends frames to the Wayland thread. It also drives typing.
  - `IndicatorPhaseTracker`, `DictationTypingSession` and `PillLayout` are pure. Test new behaviour there.
- `notulist-whisper-sys/` holds raw bindgen bindings to nixpkgs' `whisper-cpp`, found through pkg-config. The only safe wrapper is `notulist-server/src/transcribe/whisper_model_context.rs`; keep all `unsafe` there. It calls `ggml_backend_load_all()` once, because nixpkgs builds ggml with dynamically loaded backends.
- `nix/nixos-module.nix` is the only install path. It picks the package and model from the host's video drivers. Model names and SHA-256 hashes live in `notulist-server/whisper-models.toml`, read by both the daemon (for its download fallback) and `nix/whisper-models.nix` (for `fetchurl`).

## Scope

- One engine: whisper.cpp from nixpkgs with the sliding-window streamer. ONNX and other engines stay out, and whisper.cpp is never compiled in this repo.
- One transport: the session bus. No signals, state files or extra sockets.
- New typing paths are fallbacks behind the input method, never in front of it.

## Code

- No comments. Use names and call flow to explain the code. Clap help text goes in `about`/`help` attributes.
- Names use 2–4 concrete words (`start_dictation_session`, not `start` or `process`). Config keys say what they control and carry their unit (`pill_height_px`, `max_session_secs`).
- Errors are typed enums per crate (`notulist-server/src/error.rs`, `notulist-input/src/error.rs`) that wrap source errors. No `anyhow`, and no `String` payloads. Bus failures travel as `DictationFailure` variants.
- Propagate with `?`, and handle failures with `let-else`/`if let`/`match` instead of `map_err`/`and_then` chains.
- Use iterator combinators instead of `for` loops, and named structs instead of anonymous tuples.
- Split along the data flow. Don't grow one enum that every module matches on.
- Before pushing: `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run --workspace`.
- Version control is jj.

## Shells

- `nix develop` (and direnv's `use flake`) links the Vulkan whisper.cpp from cache.nixos.org, which runs on NVIDIA, AMD and Intel GPUs. `nix develop .#cuda` links the CUDA build from `cache.nixos-cuda.org`, and `nix develop .#cpu` the CPU-only build.
- Only bump this flake's nixpkgs to a revision for which `cache.nixos-cuda.org` has `whisper-cpp`. Check with `nix path-info --store https://cache.nixos-cuda.org $(nix eval --raw .#cuda.passthru.whisperCpp.outPath)`.
- Run tests in an isolated shell: `nix develop -i --keep HOME -c cargo nextest run --workspace`. An inherited `LD_LIBRARY_PATH` from another dev shell breaks binaries with `GLIBC_2.43 not found`.
- Run Wayland programs in a plain `nix develop`. With `-i`, also keep `WAYLAND_DISPLAY` and `XDG_RUNTIME_DIR`.

## Iterating on the indicator

```sh
cargo run --example popup     # the real popup in a focused field, cycling all phases
cargo run --example render    # every state as PNG in target/indicator-preview/
cargo run --example probe     # bare popup that logs pointer events
imv target/indicator-preview/*.png   # reloads when render rewrites the files
```

Stop `notulist-input` before running `popup` or `probe`. The compositor gives the input-method seat to only one client.

## Replaying speech through the whole pipeline

```sh
nix develop .#cuda
cargo run --example replay -- notulist-server/tests/fixtures/speech/speech_long.wav [--speed 1.0]
```

This uses the real capture → sliding window → whisper → edits path, with a WAV file in place of the microphone. It prints phases, decoding passes and edits (corrections marked), then the final transcript. A running `notulist-input` types the result into the focused field. Stop `notulist-server` first. Set `[daemon.streaming] revise_recent_words = true` to see corrections.

## End-to-end check with the microphone

```sh
notulist-server -v &
busctl --user introspect io.github.wvhulle.Notulist /io/github/wvhulle/Notulist
busctl --user monitor io.github.wvhulle.Notulist &
notulist-server record toggle --wait    # speak, then run `notulist-server record toggle` from another shell
```
