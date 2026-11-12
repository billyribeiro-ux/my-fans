#!/usr/bin/env bash
set -euo pipefail
media_image="${1:?Usage: verify-media-isolation.sh sha256:IMMUTABLE_IMAGE_ID}"
if [[ ! "$media_image" =~ ^sha256:[a-f0-9]{64}$ ]]; then
  printf '%s\n' 'Media isolation verification requires an immutable image ID.' >&2
  exit 2
fi
# The canary is owned by the decoder UID on a writable tmpfs. A read-only root
# filesystem or ordinary file permissions therefore cannot fake this result.
for decoder_uid in 65532 1000; do
docker run --rm --read-only --user "$decoder_uid:$decoder_uid" --cap-drop ALL --security-opt no-new-privileges \
  --network none --memory 1536m --cpus 2 --pids-limit 256 \
  --tmpfs /tmp:rw,noexec,nosuid,size=768m,mode=1777 \
  -e DATABASE_URL=nonsecret-environment-canary \
  -e AWS_CONTAINER_CREDENTIALS_RELATIVE_URI=/nonsecret-metadata-canary \
  --entrypoint /bin/sh "$media_image" -ec '
  mkdir -m 700 /tmp/work
  printf filesystem-canary > /tmp/canary
  /usr/local/bin/myfans-media-sandbox /tmp/work /usr/local/bin/myfans-media self-test /tmp/canary
  test "$(cat /tmp/canary)" = filesystem-canary
  /usr/local/bin/myfans-media-sandbox /tmp/work /usr/local/bin/ffmpeg -v error -filter_threads 1 -filter_complex_threads 1 -threads 2 -f lavfi -i color=c=blue:s=64x64:r=1 -frames:v 1 -threads 2 -c:v mjpeg -update 1 source.jpg
  /usr/local/bin/myfans-media-sandbox /tmp/work /usr/local/bin/myfans-media image source.jpg full.jpg preview.jpg
  test -s /tmp/work/full.jpg
  test -s /tmp/work/preview.jpg
  /usr/local/bin/myfans-media-sandbox /tmp/work /usr/local/bin/ffmpeg -v error -filter_threads 1 -filter_complex_threads 1 -threads 2 -f lavfi -i color=c=blue:s=1280x720:r=30 -t 1 -threads 2 -c:v libx264 clip.mp4
  /usr/local/bin/myfans-media-sandbox /tmp/work /usr/local/bin/ffprobe -v error -show_entries format=duration -of json clip.mp4
  # Run the production encoder, both HLS tiers, remux, preview and output
  # validation with audio and silent inputs. No database/provider access.
  MEDIA_ROOT=/tmp /usr/local/bin/myfans-api verify-media-pipeline
  test "$(find /tmp -maxdepth 1 -name ".work-*" | wc -l)" -eq 0
  # An argument-limit failure must come from the launcher, not FFmpeg parsing.
  set --
  count=0
  while [ "$count" -lt 254 ]; do set -- "$@" -version; count=$((count + 1)); done
  status=0
  /usr/local/bin/myfans-media-sandbox /tmp/work /usr/local/bin/ffprobe "$@" 2>/tmp/denial || status=$?
  test "$status" -eq 125
  test "$(cat /tmp/denial)" = media_sandbox_unavailable:invocation
  large=$(head -c 32768 /dev/zero | tr "\\000" x)
  status=0
  /usr/local/bin/myfans-media-sandbox /tmp/work /usr/local/bin/ffprobe "$large" 2>/tmp/denial || status=$?
  test "$status" -eq 125
  test "$(cat /tmp/denial)" = media_sandbox_unavailable:arguments
  if /usr/local/bin/myfans-media-sandbox /tmp/work /bin/sh -c true; then
    printf "%s\n" unexpected_executable_allowed >&2
    exit 1
  fi
  chmod 755 /tmp/work
  if /usr/local/bin/myfans-media-sandbox /tmp/work /usr/local/bin/ffprobe -version; then
    printf "%s\n" unsafe_workspace_allowed >&2
    exit 1
  fi
  printf "%s\n" isolated_full_media_pipeline_and_denials_verified
'
done
