# README demonstrations

These GIFs capture alloyOS's production software renderer. They are reproducible
previews, with input and elapsed time supplied by a capture harness. They are not
recordings of a live model session.

| File | Content | Duration | Dimensions | Size |
| --- | --- | --- | --- | --- |
| `liquid-boot.gif` | Real boot animation and transition out of boot state | 5 s | 480 × 1044 | 129 KB |
| `liquid-keyboard.gif` | Keyboard reveal, typed draft, held-key feedback, dismissal | 7 s | 480 × 1044 | 255 KB |
| `liquid-keyboard-detail.gif` | Cropped view of the same keyboard recording | 7 s | 480 × 564 | 234 KB |
| `setup-wizard.gif` | Real onboarding controller transitions, rendered as conversation | 22.4 s | 480 × 1044 | 321 KB |
| `memory-controls.gif` | Review demo knowledge, open keyboard, scroll, and pause learning | 12 s | 480 × 1044 | 862 KB |
| `weather-pin-sidebar.gif` | Submit weather question, sample tile, pin, and pull open the drawer | 12 s | 480 × 1044 | 741 KB |

Sizes are decimal KB. The memory and weather clips run at 12 fps with a 32-color palette; other clips run at 20 fps. The setup preview
supplies normalized answers directly to the real Welder setup controller, then
renders its conversation output. It demonstrates names, region, clock format,
learning preference, review, and completion; it does not exercise model
interpretation, provider credentials, build-tool installation, or guest boot.

The memory preview uses six neutral records in the real Welder knowledge store.
The opening frame shows the heading after a real scroll preroll. The recorded
sequence holds it for 2.5 seconds, scrolls to the final controls, opens the keyboard
through its actual button at 5.5 seconds, and activates `pause_learning` at 8.25
seconds. The callback is asserted before the capture can finish;
the host capture harness then supplies the paused tile produced by Welder. It
demonstrates review and pausing, not natural-language correction or extraction.
The memory GIF has no editorial touch indication. The tile's persistence label
is production UI text; this host preview uses an
in-memory backend and does not establish persistence across guest restarts.

The weather preview types `Weather in San Francisco?` using actual keyboard
input, sends it with Enter, verifies the exact submission, then supplies the
bundled Weather app's real MCP-rendered tile. It uses the app's initial sample
cache (San Francisco, 20°C), not a current forecast or model response. A genuine
pointer press activates the pin callback, verified through dashboard membership,
and a physical left-edge drag opens the drawer. The tile's internal `live` label
is production sample output and does not mean a live weather fetch occurred.
An editorial touch pointer marks Send at 3 seconds, moves to Pin at 6.5 seconds,
then follows the left-edge drawer drag from 8.25–9.5 seconds. It fills on contact
and fades after release. This annotation is not OS UI; Send is triggered by Enter
in the original renderer capture.

## Reproduce renderer previews

Run from the repository root:

```sh
mkdir -p /tmp/alloyos-readme-capture
cargo run -p alloyos-welder --example readme_setup_capture -- /tmp/alloyos-readme-capture/setup.json /tmp/alloyos-readme-capture/memory.json /tmp/alloyos-readme-capture/weather.json
cargo run -p alloyos-ferrofluid --example readme_capture -- boot /tmp/alloyos-readme-capture/boot
cargo run -p alloyos-ferrofluid --example readme_capture -- keyboard /tmp/alloyos-readme-capture/keyboard /tmp/alloyos-readme-capture/setup.json
cargo run -p alloyos-ferrofluid --example readme_capture -- setup /tmp/alloyos-readme-capture/setup /tmp/alloyos-readme-capture/setup.json
cargo run -p alloyos-ferrofluid --example readme_capture -- memory /tmp/alloyos-readme-capture/memory /tmp/alloyos-readme-capture/memory.json
cargo run -p alloyos-ferrofluid --example readme_capture -- weather /tmp/alloyos-readme-capture/weather /tmp/alloyos-readme-capture/weather.json
```

Encode a frame directory with a generated palette, preserving 20 fps:

```sh
ffmpeg -framerate 20 -i /tmp/alloyos-readme-capture/boot/frame-%04d.png \
  -filter_complex '[0:v]scale=480:-2:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=sierra2_4a' \
  -loop 0 assets/demos/liquid-boot.gif
```

Use the corresponding frame directory and output name for the keyboard and setup
clips. For memory and weather, use the filter below and write to
`assets/demos/memory-controls.gif` or `assets/demos/weather-pin-sidebar.gif`:

```text
[0:v]fps=12,scale=480:-2:flags=lanczos,split[a][b];[a]palettegen=max_colors=32:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle
```

Reproduce the memory GIF without touch indications and the weather GIF with its touch pointer using:

```sh
sh scripts/encode-memory-demo.sh /tmp/alloyos-readme-capture/memory
sh scripts/encode-weather-demo.sh /tmp/alloyos-readme-capture/weather
```

The keyboard detail uses `crop=iw:1416:0:1206,scale=480:-2:flags=lanczos`
in place of the full-frame scaling filter. Inspect representative frames at the
README's display size, especially the beginning/end of long messages, after any
renderer or typography change.

## Pending live recordings

- **Ask → interface → pin:** one real request, resulting tile, pinning, and
  retrieval from the left drawer. The available weather GIF is a scripted preview
  with genuine pin/drawer interactions and sample app data. The live model version
  is pending a working model provider; do not
  substitute a seeded response for a live demonstration.
- **Voice:** pending human-assisted recording by user direction. Do not substitute
  a simulated waveform for a live voice demonstration.
- **Natural-language memory correction:** pending a working model provider. The
  available memory-controls preview demonstrates review and pausing only.

`scripts/record-readme-live.py` captures a disposable QEMU guest through native
QMP screenshots. Use neutral demo content and private disposable guest state.
Never commit credentials, guest logs/disks, or personal conversation history.
