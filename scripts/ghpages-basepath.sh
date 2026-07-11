#!/usr/bin/env bash
# Rewrites the absolute /tts/ asset paths (baked into the TTS bundle, the glue and
# index.html) to include the GitHub Pages base path. Runs as a Trunk post_build
# hook only for the gh-pages config, where public_url = /epub-reader-rs/.
set -euo pipefail

dist="${TRUNK_STAGING_DIR:-${TRUNK_DIST_DIR:-dist}}"
pub="${TRUNK_PUBLIC_URL:-/}"

# Nothing to do when served from the root.
[ "$pub" = "/" ] && exit 0

for f in "$dist/index.html" "$dist/tts/piper_tts.js" "$dist/tts/piper.bundle.js"; do
  [ -f "$f" ] && sed -i "s#/tts/#${pub}tts/#g" "$f"
done
