#!/bin/bash
# build.sh [l|h] - narrate, render, mux, concat the crewboard explainer.
#   l: 480p15 draft  -> out/crewboard-explainer-480p.mp4
#   h: 1080p30 final -> out/crewboard-explainer.mp4
# Parameters (environment, all optional):
#   CB_SCRIPT     script json, default script.json
#   CB_SCENES     scenes module file, default scenes.py
#   CB_SCENELIST  name of the python list of scene classes in that module, default ALL_SCENES
#   CB_DRYRUN=1   no TTS, no audio, no mux: render only, with the committed timing.json
#   CB_ONLY="S03Trust S00Title"  limit the render to these classes (dry run)
# Every step is idempotent: it records a stamp (sha256 of its inputs) and skips when the
# stamp is unchanged, so a killed run resumes where it stopped. One log per step under log/.
# macOS bash 3.2: no declare -A, no mapfile. ls/cat are aliased in the user's shell: /bin/ls, /bin/cat.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
cd "$HERE"
SCRIPT_JSON="${CB_SCRIPT:-script.json}"
SCENES_FILE="${CB_SCENES:-scenes.py}"
SCENELIST="${CB_SCENELIST:-ALL_SCENES}"
SCENES_MOD="${SCENES_FILE%.py}"              # manim names its output dir after this module
DRYRUN="${CB_DRYRUN:-0}"
ONLY="${CB_ONLY:-}"
# narrate.sh, narrate_gemini.py and the scenes read "script.json" next to themselves: a different
# CB_SCRIPT must be copied to that name first.
[ -f "$SCRIPT_JSON" ] || { echo "build: $SCRIPT_JSON missing" >&2; exit 1; }
[ -e "$SCENES_FILE" ] || { echo "build: $SCENES_FILE missing" >&2; exit 1; }

Q="${1:-l}"
case "$Q" in
  l) RES="480p15"; FPS=15; CRF=23; OUT="out/crewboard-explainer-480p.mp4" ;;
  h) RES="1080p30"; FPS=30; CRF=18; OUT="out/crewboard-explainer.mp4" ;;  # manim 0.21 -qh defaults to 60 fps
  *) echo "usage: $0 [l|h]" >&2; exit 2 ;;
esac
MANIM=(uv run --python 3.13 --with manim manim)
PY=(uv run --python 3.13 --with manim python)
mkdir -p log out render/"$Q" mux/"$Q" audio/scene review/"$Q" text

say_step() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*"; }
fail() { echo "build: $*" >&2; exit 1; }
h_files() { /bin/cat "$@" | shasum -a 256 | cut -d' ' -f1; }
dur() { ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 "$1"; }
run_logged() {  # run_logged <log> <cmd...>; on failure show the log tail
  local lg="$1"; shift
  if ! "$@" > "$lg" 2>&1; then
    echo "build: step failed, last lines of $lg:" >&2
    tail -n 25 "$lg" >&2
    exit 1
  fi
}

# ---------------------------------------------------------------- 1. narrate
# tts.groups: a few TTS calls, scene numbers (not positions); every scene exactly once.
GROUPS_NOW="$(jq -c '.tts.groups' "$SCRIPT_JSON")"
[ "$(jq -c '[.tts.groups[][]] | sort' "$SCRIPT_JSON")" = "$(jq -c '[.scenes | keys[] | tonumber] | sort' "$SCRIPT_JSON")" ] \
  || fail "tts.groups $GROUPS_NOW do not cover every scene exactly once"
# speech estimate per group (14.95 chars/s, the measured average of the template build), informational
jq -r --argjson cps 14.95 '. as $r | .tts.groups | to_entries[] | .key as $i | .value as $g
  | "group \($i+1) scenes \($g|tostring): about \([$r.segments[] | select(.scene as $s | $g | index($s)) | (.text|length)] | add / $cps | floor) s of speech"' "$SCRIPT_JSON"
if [ "$DRYRUN" = "1" ]; then
  # no TTS: the committed timing.json (measured narration) must cover every segment
  [ -f timing.json ] || fail "dry run needs timing.json"
  [ "$(jq -r --slurpfile t timing.json '[.segments[].id | select($t[0][.] == null)] | length' "$SCRIPT_JSON")" = "0" ] \
    || fail "timing.json lacks segments of $SCRIPT_JSON"
  say_step "dry run: narrate and audio skipped, timing.json as committed"
else
  say_step "narrate"
  run_logged log/narrate.log ./narrate.sh
  grep -E 'split|largest|aligner says|narrat' log/narrate.log | tail -n 20 || true
fi

# play order from the "order" key (scene numbers); fall back to the numeric scene order when the key is missing.
if jq -e 'has("order")' "$SCRIPT_JSON" > /dev/null; then
  SCENE_NOS="$(jq -r '.order[]' "$SCRIPT_JSON")"
  # the order must hold every scene exactly once
  [ "$(echo "$SCENE_NOS" | sort -n | tr '\n' ' ')" = "$(jq -r '.scenes | keys[]' "$SCRIPT_JSON" | sort -n | tr '\n' ' ')" ] \
    || fail "order in $SCRIPT_JSON does not hold every scene exactly once"
else
  SCENE_NOS="$(jq -r '.scenes | keys[]' "$SCRIPT_JSON" | sort -n)"
fi
# the scene list of the module must match the classes in the order (sanity check, no import of manim needed)
for n in $SCENE_NOS; do
  c="$(jq -r ".scenes[\"$n\"].class" "$SCRIPT_JSON")"
  grep -q "^class $c(" "$SCENES_FILE" || fail "class $c not in $SCENES_FILE"
done
ORDER_CLASSES="$(for n in $SCENE_NOS; do jq -r ".scenes[\"$n\"].class" "$SCRIPT_JSON"; done | tr '\n' ' ')"
LIST_CLASSES="$(grep -A6 "^$SCENELIST = " "$SCENES_FILE" | tr -d '[],"\n' | sed "s/^$SCENELIST = //" | tr -s ' ' | sed 's/ *$//')"
[ "$(echo $ORDER_CLASSES)" = "$(echo $LIST_CLASSES)" ] || fail "$SCENELIST in $SCENES_FILE differs from the order of $SCRIPT_JSON"
# dry run may limit the scenes
if [ -n "$ONLY" ]; then
  keep=""; for n in $SCENE_NOS; do c="$(jq -r ".scenes[\"$n\"].class" "$SCRIPT_JSON")"; case " $ONLY " in *" $c "*) keep="$keep $n";; esac; done
  SCENE_NOS="$keep"
fi
LEAD="$(jq -r '.pacing.lead_in' "$SCRIPT_JSON")"
GAP="$(jq -r '.pacing.gap' "$SCRIPT_JSON")"
TAIL="$(jq -r '.pacing.tail' "$SCRIPT_JSON")"

# ---------------------------------------------------------------- 2. scene audio
# lead_in silence, seg 1, gap, seg 2, ..., seg N, tail silence (the layout scenes.py waits for)
# dry run has no audio
for n in $( [ "$DRYRUN" = "1" ] && echo "" || echo $SCENE_NOS ); do
  cls="$(jq -r ".scenes[\"$n\"].class" "$SCRIPT_JSON")"
  ids="$(jq -r ".segments[] | select(.scene == $n) | .id" "$SCRIPT_JSON")"
  [ -n "$ids" ] || fail "scene $n has no segments"
  files=""; for id in $ids; do files="$files audio/$id.wav"; done
  # shellcheck disable=SC2086
  stamp="$( { printf '%s|%s|%s|' "$LEAD" "$GAP" "$TAIL"; h_files $files; } | shasum -a 256 | cut -d' ' -f1)"
  wav="audio/scene/$cls.wav"
  if [ -s "$wav" ] && [ "$(/bin/cat "$wav.stamp" 2>/dev/null)" = "$stamp" ]; then continue; fi
  args=(); fc=""; k=0; last=$(( $(echo "$ids" | wc -l) - 1 ))
  for id in $ids; do
    args+=(-i "audio/$id.wav")
    pre="[$k:a]aresample=48000,aformat=channel_layouts=mono"
    [ "$k" -eq 0 ] && pre="$pre,adelay=delays=$(printf '%.0f' "$(echo "$LEAD*1000" | bc -l)")ms:all=1"
    if [ "$k" -eq "$last" ]; then pre="$pre,apad=pad_dur=$TAIL"; else pre="$pre,apad=pad_dur=$GAP"; fi
    fc="$fc$pre[a$k];"
    k=$((k+1))
  done
  j=0; cat_in=""; while [ "$j" -lt "$k" ]; do cat_in="$cat_in[a$j]"; j=$((j+1)); done
  fc="${fc}${cat_in}concat=n=$k:v=0:a=1[out]"
  say_step "scene audio $cls ($k segments)"
  run_logged "log/audio-$cls.log" ffmpeg -y -hide_banner "${args[@]}" -filter_complex "$fc" -map "[out]" -c:a pcm_s16le "$wav.tmp.wav"
  mv "$wav.tmp.wav" "$wav"
  printf '%s' "$stamp" > "$wav.stamp"
done

# ---------------------------------------------------------------- 3. render
# --disable_caching: manim's partial-movie cache added up to 0.3 s per scene when partial files
# from different runs were stitched; a fresh render keeps the video on the planned timeline.
MFLAGS="--disable_caching --progress_bar none --frame_rate $FPS"
RSTAMP="$( { printf '%s|' "$MFLAGS"; h_files "$SCENES_FILE" "$SCRIPT_JSON" timing.json; } | shasum -a 256 | cut -d' ' -f1)"
for n in $SCENE_NOS; do
  cls="$(jq -r ".scenes[\"$n\"].class" "$SCRIPT_JSON")"
  dst="render/$Q/$cls.mp4"
  if [ -s "$dst" ] && [ "$(/bin/cat "$dst.stamp" 2>/dev/null)" = "$RSTAMP" ]; then continue; fi
  say_step "render $cls ($RES)"
  run_logged "log/render-$Q-$cls.log" "${MANIM[@]}" "-q$Q" $MFLAGS --media_dir media "$SCENES_FILE" "$cls"
  src="media/videos/$SCENES_MOD/$RES/$cls.mp4"   # dir is named after the module
  [ -s "$src" ] || fail "manim produced no $src"
  grep -h "TIMING" "log/render-$Q-$cls.log" || true
  cp "$src" "$dst"
  printf '%s' "$RSTAMP" > "$dst.stamp"
done

# a dry run stops here: no audio, so no mux, concat or review frames
if [ "$DRYRUN" = "1" ]; then
  for n in $SCENE_NOS; do
    cls="$(jq -r ".scenes[\"$n\"].class" "$SCRIPT_JSON")"
    echo "dry run: render/$Q/$cls.mp4 duration $(dur "render/$Q/$cls.mp4") s"
  done
  exit 0
fi
# ---------------------------------------------------------------- 4. mux
# Narration is never cut: the result is max(audio, video) long; the video is padded with its
# last frame, the audio with silence. Audio and video must agree within 0.5 s before padding.
for n in $SCENE_NOS; do
  cls="$(jq -r ".scenes[\"$n\"].class" "$SCRIPT_JSON")"
  v="render/$Q/$cls.mp4"; a="audio/scene/$cls.wav"; m="mux/$Q/$cls.mp4"
  stamp="$(h_files "$v" "$a")"
  if [ -s "$m" ] && [ "$(/bin/cat "$m.stamp" 2>/dev/null)" = "$stamp" ]; then continue; fi
  A="$(dur "$a")"; V="$(dur "$v")"
  ok="$(echo "d=$A-$V; if (d<0) d=-d; d<=0.5" | bc -l)"
  [ "$ok" = "1" ] || fail "$cls: audio ${A}s vs video ${V}s differ by more than 0.5 s"
  L="$(printf '%.3f' "$(echo "if ($A>$V) $A else $V" | bc -l)")"
  PAD="$(printf '%.3f' "$(echo "$L-$V+0.1" | bc -l)")"
  say_step "mux $cls (audio ${A}s, video ${V}s)"
  run_logged "log/mux-$Q-$cls.log" ffmpeg -y -hide_banner -i "$v" -i "$a" \
    -filter_complex "[0:v]tpad=stop_mode=clone:stop_duration=$PAD,format=yuv420p[v];[1:a]apad[a]" \
    -map "[v]" -map "[a]" -t "$L" -c:v libx264 -preset medium -crf "$CRF" -pix_fmt yuv420p \
    -c:a aac -b:a 192k -ar 48000 -movflags +faststart "$m.tmp.mp4"
  mv "$m.tmp.mp4" "$m"
  printf '%s' "$stamp" > "$m.stamp"
done

# ---------------------------------------------------------------- 5. concat + chapters
list="mux/$Q/concat.txt"; meta="mux/$Q/chapters.txt"
: > "$list.new"
printf ';FFMETADATA1\ntitle=crewboard - a crew of agents, one board, proof before Done\n' > "$meta.new"
t=0
pos=0   # chapter number = position in the play order, not the scene number
for n in $SCENE_NOS; do
  cls="$(jq -r ".scenes[\"$n\"].class" "$SCRIPT_JSON")"
  title="$(jq -r ".scenes[\"$n\"].title" "$SCRIPT_JSON")"
  m="mux/$Q/$cls.mp4"
  printf "file '%s'\n" "$cls.mp4" >> "$list.new"
  d="$(dur "$m")"
  s_ms="$(echo "$t*1000/1" | bc)"; t="$(echo "$t+$d" | bc -l)"; e_ms="$(echo "$t*1000/1" | bc)"
  if [ "$pos" -eq 0 ]; then ctitle="$title"; else ctitle="$pos. $title"; fi  # position 0 is the title card; was "$n"
  pos=$((pos+1))
  printf '[CHAPTER]\nTIMEBASE=1/1000\nSTART=%s\nEND=%s\ntitle=%s\n' "$s_ms" "$e_ms" "$ctitle" >> "$meta.new"
done
mv "$list.new" "$list"; mv "$meta.new" "$meta"
files=""; for n in $SCENE_NOS; do files="$files mux/$Q/$(jq -r ".scenes[\"$n\"].class" "$SCRIPT_JSON").mp4"; done
# shellcheck disable=SC2086
stamp="$(h_files $files "$meta")"
if [ -s "$OUT" ] && [ "$(/bin/cat "$OUT.stamp" 2>/dev/null)" = "$stamp" ]; then
  say_step "concat unchanged"
else
  say_step "concat -> $OUT"
  run_logged "log/concat-$Q.log" ffmpeg -y -hide_banner -f concat -safe 0 -i "$list" -i "$meta" \
    -map 0 -map_metadata 1 -map_chapters 1 -c copy -movflags +faststart "$OUT.tmp.mp4"
  mv "$OUT.tmp.mp4" "$OUT"
  printf '%s' "$stamp" > "$OUT.stamp"
fi

# ---------------------------------------------------------------- 6. review frames + timing check
for n in $SCENE_NOS; do
  cls="$(jq -r ".scenes[\"$n\"].class" "$SCRIPT_JSON")"
  v="render/$Q/$cls.mp4"
  st="review/$Q/$cls.stamp"; vs="$(h_files "$v")"
  if [ -s "review/$Q/$cls-40.png" ] && [ "$(/bin/cat "$st" 2>/dev/null)" = "$vs" ]; then continue; fi
  V="$(dur "$v")"
  at="$(printf '%.3f' "$(echo "$V*0.4" | bc -l)")"; lt="$(printf '%.3f' "$(echo "$V-0.15" | bc -l)")"
  run_logged "log/review-$Q-$cls.log" ffmpeg -y -hide_banner -ss "$at" -i "$v" -frames:v 1 "review/$Q/$cls-40.png"
  ffmpeg -y -hide_banner -ss "$lt" -i "$v" -frames:v 1 "review/$Q/$cls-end.png" >> "log/review-$Q-$cls.log" 2>&1
  printf '%s' "$vs" > "$st"
done
say_step "per-scene lengths (audio / rendered video / muxed)"
for n in $SCENE_NOS; do
  cls="$(jq -r ".scenes[\"$n\"].class" "$SCRIPT_JSON")"
  A="$(dur "audio/scene/$cls.wav")"; V="$(dur "render/$Q/$cls.mp4")"; M="$(dur "mux/$Q/$cls.mp4")"
  printf '  %-22s audio %8.3f  video %8.3f  muxed %8.3f  diff %+.3f\n' "$cls" "$A" "$V" "$M" "$(echo "$V-$A" | bc -l)"
done
say_step "result $OUT"
ffprobe -v error -show_entries stream=codec_type,codec_name,width,height,r_frame_rate,sample_rate,channels -of compact "$OUT"
echo "chapters: $(ffprobe -v error -show_chapters -of csv "$OUT" | grep -c '^chapter')"
echo "duration: $(dur "$OUT") s"
echo "sha256:   $(shasum -a 256 "$OUT" | cut -d' ' -f1)"
