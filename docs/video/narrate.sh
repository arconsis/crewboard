#!/bin/bash
# narrate.sh - one WAV per narration segment (Gemini TTS or macOS say), durations into timing.json.
# Idempotent: a segment whose text+voice+rate hash is unchanged is skipped.
# macOS bash 3.2: no declare -A, no mapfile. Text goes to say via a file (say -f), never argv.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
cd "$HERE"
mkdir -p audio text

# Backend: script.json tts.backend "gemini" hands over to narrate_gemini.py (key from the
# environment, one call per scene group); anything else uses macOS say, one WAV per segment.
if [ "$(jq -r '.tts.backend // "say"' script.json)" = "gemini" ]; then
  exec uv run --python 3.13 python narrate_gemini.py
fi

VOICE="$(jq -r '.voice' script.json)"
RATE="$(jq -r '.rate' script.json)"
[ -n "$VOICE" ] && [ "$VOICE" != "null" ] || { echo "narrate: no voice in script.json" >&2; exit 1; }

N="$(jq '.segments | length' script.json)"
[ "$N" -gt 0 ] || { echo "narrate: no segments" >&2; exit 1; }

made=0; kept=0
i=0
while [ "$i" -lt "$N" ]; do
  id="$(jq -r ".segments[$i].id" script.json)"
  case "$id" in s[0-9]*-[0-9]*) ;; *) echo "narrate: bad id '$id'" >&2; exit 1;; esac
  jq -j ".segments[$i].text" script.json > "text/$id.txt.new"
  [ -s "text/$id.txt.new" ] || { echo "narrate: empty text for $id" >&2; exit 1; }
  h="$( { printf '%s|%s|' "$VOICE" "$RATE"; /bin/cat "text/$id.txt.new"; } | shasum -a 256 | cut -d' ' -f1)"
  if [ -s "audio/$id.wav" ] && [ -f "audio/$id.wav.hash" ] && [ "$(/bin/cat "audio/$id.wav.hash")" = "$h" ]; then
    kept=$((kept+1)); rm -f "text/$id.txt.new"
  else
    mv "text/$id.txt.new" "text/$id.txt"
    rm -f "audio/$id.wav" "audio/$id.wav.hash"
    say -v "$VOICE" -r "$RATE" --file-format=WAVE --data-format=LEI16@22050 -o "audio/$id.wav.tmp.wav" -f "text/$id.txt"
    mv "audio/$id.wav.tmp.wav" "audio/$id.wav"
    printf '%s' "$h" > "audio/$id.wav.hash"
    made=$((made+1))
    echo "narrate: rendered $id"
  fi
  i=$((i+1))
done

# Durations: ffprobe every segment; the scenes size their waits from this file.
tmp="timing.json.tmp"
printf '{\n' > "$tmp"
i=0
while [ "$i" -lt "$N" ]; do
  id="$(jq -r ".segments[$i].id" script.json)"
  d="$(ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 "audio/$id.wav")"
  case "$d" in [0-9]*.[0-9]*) ;; *) echo "narrate: ffprobe gave '$d' for $id" >&2; exit 1;; esac
  sep=","; [ "$i" -eq $((N-1)) ] && sep=""
  printf '  "%s": %s%s\n' "$id" "$d" "$sep" >> "$tmp"
  i=$((i+1))
done
printf '}\n' >> "$tmp"
jq -e 'type == "object" and length > 0' "$tmp" > /dev/null || { echo "narrate: timing.json is not a valid object" >&2; exit 1; }
cnt="$(jq 'length' "$tmp")"
[ "$cnt" -eq "$N" ] || { echo "narrate: timing has $cnt entries, script has $N" >&2; exit 1; }
if [ -f timing.json ] && cmp -s "$tmp" timing.json; then rm -f "$tmp"; else mv "$tmp" timing.json; fi
total="$(jq '[.[]] | add' timing.json)"
echo "narrate: $N segments ($made rendered, $kept unchanged), narration total ${total}s"
