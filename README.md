# notulist

Streaming dictation for Wayland. A daemon listens to the microphone and transcribes with whisper.cpp while you speak. A separate input method types the text into the focused field as preedit, and shows a small indicator at the text cursor with the current phase and a live waveform.

notulist is a slimmed-down fork of [voxtype](https://github.com/peteonrails/voxtype). See `NOTICE`.

## Parts

| Crate | Binary | Job |
|---|---|---|
| `notulist-daemon` | `notulist` | Audio capture, sliding-window whisper streaming, text processing, D-Bus service |
| `notulist-ime` | `notulist-ime` | Wayland input method (`zwp_input_method_v2`), preedit and commit, dotool fallback, indicator popup |
| `notulist-client` | library | The D-Bus contract: types, proxy and a transcript accumulator |
| `notulist-whisper-sys` | library | Raw bindings to the system whisper.cpp (nixpkgs `whisper-cpp`), found through pkg-config |

whisper.cpp is not compiled here. The daemon links the prebuilt `whisper-cpp` from nixpkgs and loads its ggml backends (CPU variants, CUDA or Vulkan) at runtime.

They talk over the session bus as `io.github.wvhulle.Notulist`, interface `io.github.wvhulle.Notulist.Dictation1`:

- methods: `Start`, `Stop`, `Toggle`, `Cancel`, `CurrentPhase`
- signals: `PhaseChanged(s)`, `Levels(a(uddd))`, `TranscriptEdited((us))`, `DecodingChanged(b)`, `SessionFailed(s)`, `SessionEnded(s)`
- daemon phases: `LoadingModel`, `Idle`, `Listening`, `Finishing`, `Failed`
- failures: `MicrophoneUnavailable`, `MicrophoneDisconnected`, `ModelNotLoaded`, `InferenceFailed`

A transcript edit means "delete this many characters from the end of what this session typed, then insert this text". The daemon applies `[daemon.text_cleanup]` to final segments before sending them.

The indicator popup shows these phases: daemon offline, loading model, ready, failed, waiting for speech, hearing, decoding, a short correcting flash, finishing, a short committed or cancelled flash, and password field.

## Usage

```sh
notulist daemon                 # run the daemon; owns the bus name
notulist-ime                    # run the input method in your Wayland session
notulist record toggle          # bind this to a key in your compositor
notulist record toggle --wait   # block and print the transcript, no notulist-ime needed
notulist status --follow        # one line per phase change: loading_model, idle, listening, finishing, failed
notulist transcribe file.wav    # one-shot transcription
```

niri example:

```kdl
binds {
    Mod+V { spawn "notulist" "record" "toggle"; }
}
```

When the focused application has no text-input-v3 support, `notulist-ime` types through `dotool` instead. It does not type into password or PIN fields.

## Configuration

Both binaries read `~/.config/notulist/config.toml`, or the file given with `--config`/`NOTULIST_CONFIG`. The daemon reads only the `[daemon.*]` tables and the input method only the `[ime.*]` tables. Every key is optional; the values below are the defaults unless marked as an example.

```toml
[daemon.microphone]
device_name = "default"
capture_sample_rate_hz = 16000
max_session_secs = 300

[daemon.whisper]
model = "large-v3-turbo"                 # example; a name under ~/.local/share/notulist/models or a ggml .bin path
languages = ["en", "nl"]                 # example; "en", "auto", or a list to detect from
translate_to_english = false
inference_threads = 4                    # example; defaults to min(cpus, 4)
gpu_device_index = 0                     # example
flash_attention = false
shrink_context_for_short_audio = false
initial_prompt = "notulist, niri"        # example

[daemon.streaming]
retranscribe_interval_secs = 0.8
max_window_secs = 29.0
min_window_rms_for_speech = 0.005
min_audio_before_first_pass_secs = 1.0
min_new_words_per_partial = 1
type_partials_live = true
revise_recent_words = false

[daemon.text_cleanup]
spoken_punctuation = false
word_replacements = { "vox type" = "notulist" }   # example
remove_filler_words = true

[ime.typing]
fall_back_to_dotool = true
wait_for_modifier_release_ms = 2000

[ime.popup]
show_popup = true
animations_enabled = true
pill_height_px = 22
inner_padding_px = 4
corner_radius_px = 6
spacing_between_elements_px = 4
background_colour = "#1e1e22e6"          # #RRGGBB or #RRGGBBAA
font_file = "/path/to/font.ttf"          # example; fc-match sans otherwise

[ime.popup.mic_button]
show_mic_button = true
mic_button_size_px = 14

[ime.popup.level_meter]
show_level_meter = true
bar_count = 12
bar_width_px = 2
spacing_between_bars_px = 1
visible_history_ms = 600
silence_floor_dbfs = -60

[ime.popup.phase_label]
show_phase_label = true
font_size_px = 11

[ime.popup.speech_detection]
speech_loudness_threshold_dbfs = -40
keep_hearing_after_speech_ms = 300

[ime.popup.flashes]
correction_flash_ms = 400
session_outcome_flash_ms = 800

[ime.popup.phase_styles.hearing]         # example; any phase below can be restyled
label_text = "Hearing"
colour = "#3b82f6"
```

Phase style tables: `daemon_offline`, `loading_model`, `ready`, `failed`, `waiting_for_speech`, `hearing`, `decoding`, `correcting`, `finishing`, `committed`, `cancelled`, `password_field`.

## Installing on NixOS

The flake exports one NixOS module. It installs the package matching your GPU, downloads the whisper model, writes the config, and runs `notulist daemon` and `notulist-ime` as systemd user services in every graphical session, for every user.

```nix
{
  inputs.notulist.url = "github:wvhulle/notulist";

  # in your nixosSystem modules:
  #   inputs.notulist.nixosModules.default
  #   {
  #     programs.notulist = {
  #       enable = true;
  #       settings.daemon.whisper.languages = [ "en" "nl" ];
  #     };
  #   }
}
```

- `acceleration` defaults to `cuda` when `services.xserver.videoDrivers` contains `nvidia`, to `vulkan` with `amdgpu`, otherwise to `cpu`.
- `whisperModel` defaults to `large-v3-turbo`, `small` or `base` to match that.
- With `cuda`, the module trusts `cache.nixos-cuda.org` (`useCudaBinaryCache`, on by default), so the CUDA whisper.cpp is downloaded prebuilt.
- Don't make notulist's nixpkgs follow yours: notulist pins a nixpkgs revision for which the CUDA cache has whisper.cpp.
- A `uaccess` udev rule lets whoever is logged in at the seat use `/dev/uinput`, which dotool needs.

Bind a key in your compositor to `notulist record toggle`.

## Building

```sh
nix build .#cpu
nix build .#cuda       # needs cache.nixos-cuda.org, or it compiles whisper.cpp with CUDA
nix build .#vulkan
```

## Developing

`nix develop` links the Vulkan build of whisper.cpp (prebuilt on cache.nixos.org, runs on any recent GPU). `nix develop .#cuda` and `nix develop .#cpu` link the other builds. There are no Cargo features for this: the shell decides which whisper.cpp pkg-config finds. Every shell includes cargo-nextest, imv and dotool.

```sh
cargo test --workspace            # all tests
cargo run --example popup         # the real indicator in your focused text field, cycling every phase
cargo run --example render        # every indicator state as PNG in target/indicator-preview/
cargo run --example probe         # bare popup that logs pointer events
cargo run --example replay -- notulist-daemon/tests/fixtures/speech/speech_long.wav
```

`replay` runs the real daemon pipeline on a WAV file played back in real time (`--speed 2.0` to go faster). It prints a timeline of phase changes, decoding passes and each edit (corrections are marked), then the final transcript. With `notulist-ime` running and a field focused, the text is typed there and the popup goes through every phase. Stop `notulist daemon` first, since `replay` needs the bus name. Use `nix develop .#cuda`: on CPU, large-v3-turbo is far slower than real time.

`popup` and `probe` take over the input-method seat. Stop `notulist-ime` (or voxtype) first, and restart it afterwards.

To look at the rendered states, open `imv target/indicator-preview/*.png`. imv reloads files when `render` rewrites them.
