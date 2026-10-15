#!/bin/sh
# Editorial touch pointer: Send (3s), Pin (6.5s), drawer drag (8.25–9.5s).
set -eu
frames=${1:?Usage: encode-weather-demo.sh FRAME_DIRECTORY [OUTPUT_GIF]}
output=${2:-assets/demos/weather-pin-sidebar.gif}
# Coordinates refer to the 480px export. The circle stays visible between
# actions, fills on contact, and follows the physical sidebar drag.
ffmpeg -v error -framerate 20 -i "$frames/frame-%04d.png" \
  -filter_complex "[0:v]fps=12,scale=480:-2:flags=lanczos[base];color=white:s=80x80:r=12:d=12,format=rgba,geq=r=255:g=255:b=255:a='if(between(T,2.25,9.9),min(1,(T-2.25)/0.25)*min(1,(9.9-T)/0.3)*if(between(hypot(X-40,Y-40),12+8*max(0,1-abs(T-3)/0.25)+8*max(0,1-abs(T-6.5)/0.25),15+8*max(0,1-abs(T-3)/0.25)+8*max(0,1-abs(T-6.5)/0.25)),235,if(lte(hypot(X-40,Y-40),5)*(between(T,2.9,3.15)+between(T,6.4,6.65)+between(T,8.25,9.5)),200,0)),0)'[pointer];[base][pointer]overlay=x='if(lt(t,3.25),426,if(lt(t,5.75),426-18*(t-3.25)/2.5,if(lt(t,7.1),408,if(lt(t,8.15),408-406*(t-7.1)/1.05,if(lt(t,8.25),2,if(lt(t,9.5),2+388*(t-8.25)/1.25,390))))))-40':y='if(lt(t,3.25),1008,if(lt(t,5.75),1008-798*(t-3.25)/2.5,if(lt(t,7.1),210,if(lt(t,8.15),210+268*(t-7.1)/1.05,478))))-40':eval=frame:shortest=1,split[a][b];[a]palettegen=max_colors=32:stats_mode=diff[p];[b][p]paletteuse=dither=none:diff_mode=rectangle" \
  -loop 0 -y "$output"
