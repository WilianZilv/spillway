#!/usr/bin/env bash
# Small, complete MP4 clips for the demo traffic (valid for the parser).
set -e
out=${1:-/root/spw/demofix}
mkdir -p "$out"
ff="ffmpeg -hide_banner -loglevel error -y"
if [ ! -f "$out/.done" ]; then
frag="-movflags +frag_keyframe+empty_moov+default_base_moof"
fast="-movflags +faststart"
i=0
for spec in "1920x1080 libx265 6 frag" "1920x1080 libx264 4 fast" "1280x720 libx264 10 frag" "1280x720 libx265 8 fast" \
            "1920x1080 libx264 2 frag" "640x360 libx264 6 fast" "1920x1080 libx265 10 frag" "1280x720 libx264 3 fast"; do
  set -- $spec
  i=$((i + 1))
  flags=$frag; [ "$4" = fast ] && flags=$fast
  tag=""; [ "$2" = libx265 ] && tag="-tag:v hvc1 -x265-params log-level=error"
  $ff -f lavfi -i "testsrc2=size=$1:rate=15:duration=$3" -c:v $2 -preset ultrafast -b:v 450k $tag $flags "$out/clip$i.mp4"
done
touch "$out/.done"
fi

# one large valid clip for the thief scene (~60 MB)
big=${2:-/root/spw/demobig}
mkdir -p "$big"
[ -f "$big/big.mp4" ] || $ff -f lavfi -i "testsrc2=size=1920x1080:rate=30:duration=20" -c:v libx264 -preset ultrafast -b:v 24M -maxrate 24M -bufsize 8M -movflags +faststart "$big/big.mp4"
