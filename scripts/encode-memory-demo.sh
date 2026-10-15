#!/bin/sh
# Encode the actual memory renderer frames without editorial touch indicators.
set -eu
frames=${1:?Usage: encode-memory-demo.sh FRAME_DIRECTORY [OUTPUT_GIF]}
output=${2:-assets/demos/memory-controls.gif}
ffmpeg -v error -framerate 20 -i "$frames/frame-%04d.png" \
  -filter_complex '[0:v]fps=12,scale=480:-2:flags=lanczos,split[a][b];[a]palettegen=max_colors=32:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle' \
  -loop 0 -y "$output"
