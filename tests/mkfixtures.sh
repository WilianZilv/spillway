#!/usr/bin/env bash
# Real MP4 fixtures: progressive (moov at end), faststart and MediaMTX-style fragmented.
set -e
OUT=${1:-/root/spw/fixtures}
mkdir -p "$OUT"
[ -f "$OUT/frag.mp4" ] && exit 0
ff="ffmpeg -hide_banner -loglevel error -y"
$ff -f lavfi -i testsrc2=size=1280x720:rate=25 -f lavfi -i sine=frequency=440:sample_rate=48000 -t 10 \
    -c:v libx264 -preset ultrafast -b:v 1500k -g 50 -c:a aac -b:a 64k "$OUT/progressive.mp4"
$ff -f lavfi -i testsrc2=size=1280x720:rate=25 -t 10 -c:v libx264 -preset ultrafast -b:v 1500k -g 50 \
    -movflags +faststart "$OUT/faststart.mp4"
$ff -f lavfi -i testsrc2=size=1920x1080:rate=30 -f lavfi -i sine=frequency=880:sample_rate=48000 -t 12 \
    -c:v libx265 -preset ultrafast -x265-params log-level=none -b:v 2000k -g 30 -tag:v hvc1 -c:a aac \
    -movflags +frag_keyframe+empty_moov+default_base_moof "$OUT/frag.mp4"
ls -la "$OUT"
