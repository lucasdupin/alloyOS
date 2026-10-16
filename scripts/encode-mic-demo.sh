#!/bin/sh
# Match the keyboard detail crop using real renderer frames with simulated input.
set -eu
frames=${1:?Usage: encode-mic-demo.sh FRAME_DIRECTORY [OUTPUT_GIF]}
output=${2:-assets/demos/mic-detail.gif}
ffmpeg -v error -framerate 20 -i "$frames/frame-%04d.png" \
  -filter_complex '[0:v]crop=iw:1416:0:1206,scale=480:-2:flags=lanczos,split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=sierra2_4a' \
  -loop 0 -y "$output"
