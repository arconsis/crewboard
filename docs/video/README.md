# The crewboard explainer video

A short animated explainer of crewboard: a crew of agents, one board, proof before Done. This
directory holds the source. The rendered MP4 is not in the repository.

## What it is and who it is for

The audience is an engineer who runs AI coding agents and wants to know how several of them can
work as a team with a record that holds. Nothing is run in the video. Each idea is shown as a
mechanism: a message that travels to the board, a signature check, a Status column that moves, a
board compared with the signed log.

The video opens with a title card, then thirteen scenes: the cold open, the idea (one board, one
signed log), what makes a message count, the owner's setup, the owner's day, the coordinator and
a ticket with a Gate, verify and merge, the dev agent's work, how an agent joins and works
through the skill, proof before Done, the board against the signed log, strengths and limits,
and where to start. English narration, English on-screen text.

The narration speaks only what the docs of this repository say. `STORYBOARD.md` names the
source file and line of every claim. It names no host, account or customer: only the example
names of the docs (`claude@host-b`, `you/my-repo`, `teammate`).

## Files

| File | Role |
|---|---|
| `script.json` | The narration, split into segments; the scene list, the play order (`order`), the pacing and the voice settings (`tts`: backend, model, voice, director's notes, scene groups) |
| `narrate.sh` | Hands over to `narrate_gemini.py` when `tts.backend` is `gemini`; otherwise renders one WAV per segment with macOS `say` |
| `narrate_gemini.py` | Speaks each group of scenes in one Gemini TTS call and splits the answer into one WAV per segment |
| `align_gemini.py` | Asks a Gemini text model where each paragraph starts and ends; the split uses it as its second reader |
| `timing.json` | The measured segment durations. The narration step regenerates it; the scenes read it |
| `transcribe_gemini.py` | Asks a Gemini model for a transcript of narration audio; a check on what was spoken |
| `STORYBOARD.md` | The storyboard: the text of every scene, what is on screen, and the claim tables that name the source of every claim |
| `scenes.py` | The title card and the thirteen Manim scenes (`ALL_SCENES` is the play order). A scene never guesses a duration: it waits as long as its segment |
| `build.sh` | Narrate, render, mux, concatenate. Every step is idempotent and resumes after a kill |

The video follows the voice, never the other way round.

## Build it on a Mac

Prerequisites: `uv`, `ffmpeg` (with `ffprobe`), `jq`, and `pkgconf` (`brew install pkgconf`).
Manim builds `pycairo`, and `pycairo` needs `pkg-config`. No LaTeX is needed: the scenes use
`Text` only. The Gemini voice needs `GEMINI_API_KEY` in the environment, exported in your login
shell. The key goes only into the request header; it is never printed, logged or written to a
file.

```bash
brew install uv ffmpeg jq pkgconf        # once
cd docs/video
./build.sh l                              # 480p15 draft  -> out/crewboard-explainer-480p.mp4
./build.sh h                              # 1080p30 final -> out/crewboard-explainer.mp4
```

`build.sh` runs these steps, each logged under `log/`. One scene by hand:

```bash
uv run --python 3.13 --with manim manim -ql --disable_caching --media_dir media scenes.py S03Trust
```

The scenes run under `uv run --python 3.13 --with manim python`. Name every third-party package
on the command line: a bare interpreter differs from one context to the next.

Draft first, then final: set `tts.backend` to `say` in `script.json`, render at 480p, look at
the review PNGs under `review/l/` (one partway through and one at the end of each scene) for
overflow, fix the scenes, and only when the text is final set the backend to `gemini`, narrate
once, and render at 1080p. A long render runs best in the background and under `nice`.

## The voice

The narrator is Gemini TTS, model `gemini-3.8-flash-tts`, voice Sulafat, directed as a calm,
warm, unhurried documentary narrator (`tts.style` in `script.json`). The director's notes also
say how to pronounce crewboard, CLI, CI, SHA, `AUTHORITY.md`, `claude@host-b`, codex and hermes.

The free tier allows only a few TTS requests per model and day, so the narration is spoken in
four calls, one per group of scenes (`tts.groups`), with a pause between paragraphs. The answer
is cut into segments at those pauses. Two readers choose each cut: the silence detector lists
the pauses, and a Gemini text model says where each paragraph ends. The pause nearest to that
answer is the cut. The aligner drifts on long audio, so a boundary that does not check out is
aligned again on the remaining audio only. The narration step prints the split table, and it
refuses when a paragraph was not read completely or a boundary has no pause near it. On HTTP
429 (quota) it stops without retrying; finished groups are cached by content hash, so a rerun
calls only for the missing groups.

`CB_TTS_MODEL` overrides the model for one run, for example `gemini-3.8-flash-lite-tts` for a
draft. To build without a network, set `tts.backend` to `say`: macOS `say` then renders one WAV
per segment with the `voice` and `rate` in `script.json`.

Listen to the whole narration before you release the video: the pronunciation of terms such as
SHA, `AUTHORITY.md` and the agent names is not checked by any gate.

## The MP4 is a release asset

The rendered files are build outputs. `.gitignore` here excludes `audio/`, `media/`, `render/`,
`mux/`, `out/`, `review/`, `log/`, `text/`, `*.aiff`, `*.wav`, `*.mp4` and `*.png`. The finished
MP4 is handed over as a file and later as a release asset. Nothing is hosted.

## Changing the narration

Edit `script.json`, then run `./build.sh`. The narration step calls again only for the groups
whose text changed and rewrites `timing.json`. A changed word changes the video: the MP4 already
delivered was built from the text now in `script.json`. Update the claim table in
`STORYBOARD.md` with the text.

## Build log

### v1.1 (2026-10-30)

Matches crewboard 0.8.0 (`main` at `ca3fb47`), where a usage error exits 64 for every command.
Segment s9-4 gains one sentence: "Code sixty-four: the command line was wrong; fix it and run it
again." Scene 9 shows a fourth exit row, and the exit rows now share one text column. The claim
table cites `skill/SKILL.md` line 82 and `docs/reference/exit-codes.md` lines 16 and 23 to 24 at
`ca3fb47`. Nothing else changed.

Only the group that holds scene 9 was spoken again: one TTS call for group 3 (scenes 7, 8 and 9)
and one aligner call. Groups 1, 2 and 4 came from the cache. Scenes 7 and 8 therefore have new
audio with the same text.

| Item | Value |
|---|---|
| Narration | 52 segments, 7,615 characters, 517.8 s of speech; group 3: 168.5 s, largest distance from the aligner to a pause 0.31 s |
| Speech rate | scenes 7 to 9: 13.6 to 16.3 characters per second |
| Scenes | 14 chapters, 1080p30; audio and video differ by 0.033 s or less in every scene |
| `out/crewboard-explainer.mp4` | 563.2 s, 27,511,165 bytes, 1920x1080, 30 fps, H.264, AAC mono 48 kHz |
| `out/crewboard-explainer-480p.mp4` | 563.5 s, the same narration and scenes at 480p15 |

### v1 (2026-10-30)

A title card and thirteen scenes, 52 segments, 7,545 characters of narration.

The draft was built with the `say` backend at 480p (`./build.sh l`, 526.6 s). The review frames
showed overflow and layout errors, all fixed before the narration: lines that crossed the labels
of the binding (now a brace), commands too long for the terminal panel (now two lines), a card
that landed in the wrong column (`slot()` now reads the column itself, so a shifted board stays
right), two animations on one card (now two plays), document icons whose lines ran out of the box,
and overlaps in scenes 5, 10, 11 and 12. Then the backend was switched to `gemini` and the text was
narrated once.

Build of 2026-10-30 (`./build.sh h`, under `nice -n 10`):

| Item | Value |
|---|---|
| Narration | Gemini TTS, 4 groups (4 calls), 52 segments, 507.7 s of speech |
| Aligner | largest distance from the aligner to a pause: 0.86 s; groups 3 and 4 used the timed transcript, because the JSON answer ended after the audio |
| Speech rate | 11 to 19 characters per second in every segment, except the title (10.0) |
| Scenes | 14 chapters (title card and 13 scenes), 1080p30; audio and video differ by 0.03 s or less in every scene |
| `out/crewboard-explainer.mp4` | 553.2 s, 27,098,298 bytes, 1920x1080, 30 fps, H.264, AAC mono 48 kHz |

The render is not in the repository. It stays in the `out/` folder of the build machine's working
copy of this directory. The v1 files there were replaced by the v1.1 build.
