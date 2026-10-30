"""narrate_gemini.py - narration with Gemini TTS, one API call per group of scenes.

Why groups: the free tier allows few requests per model per day, so the segments are
spoken in a handful of calls (script.json tts.groups). Each call gets the segments as
paragraphs; the director's notes ask for a clear pause between paragraphs. The returned
audio is split at those pauses: every boundary is predicted from the text length and
snapped to the longest silence near the prediction. The split is printed as a table and
refuses when a boundary has no silence near it.

The key comes from GEMINI_API_KEY in the environment and goes only into the request
header; it is never printed, logged, or put into a URL. On HTTP 429 the script stops
without retrying and says when the quota allows the next call; finished groups are
cached by content hash, so a rerun only calls for the missing groups.

Usage: python narrate_gemini.py [--model M] [--only-group K] [--outdir audio]
stdlib only; needs ffmpeg and ffprobe on PATH.
"""
import argparse
import base64
import hashlib
import json
import os
import re
import subprocess
import sys
import urllib.error
import urllib.request

from align_gemini import align, align_by_transcript

HERE = os.path.dirname(os.path.abspath(__file__))
API = "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"


def die(msg, code=1):
    print(f"narrate-gemini: {msg}", file=sys.stderr)
    sys.exit(code)


def prompt_for(tts, texts):
    return (tts["style"].rstrip() + "\n\n#### TRANSCRIPT\n" + "\n\n".join(texts) + "\n")


def call_tts(model, voice, prompt, key):
    body = {
        "contents": [{"role": "user", "parts": [{"text": prompt}]}],
        "generationConfig": {
            "responseModalities": ["AUDIO"],
            "speechConfig": {"voiceConfig": {"prebuiltVoiceConfig": {"voiceName": voice}}},
        },
    }
    req = urllib.request.Request(API.format(model=model), data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json", "x-goog-api-key": key})
    try:
        with urllib.request.urlopen(req, timeout=600) as r:
            d = json.load(r)
    except urllib.error.HTTPError as e:
        raw = e.read().decode(errors="replace")
        if e.code == 429:
            delay = re.search(r'"retryDelay":\s*"([0-9.]+)s"', raw)
            quota = re.search(r'"quotaId":\s*"([^"]+)"', raw)
            die(f"HTTP 429 quota exhausted ({quota.group(1) if quota else 'quota id unknown'}); "
                f"next call allowed in {delay.group(1) if delay else '?'} s. Not retrying.", 3)
        die(f"HTTP {e.code}: {raw[:400]}")
    if not isinstance(d, dict) or not d.get("candidates"):
        die(f"unexpected response shape: {type(d).__name__} keys={list(d)[:5] if isinstance(d, dict) else '-'}")
    cand = d["candidates"][0]
    parts = cand.get("content", {}).get("parts", [])
    blobs = [p["inlineData"] for p in parts if "inlineData" in p]
    if len(blobs) != 1:
        die(f"expected one audio part, got {len(blobs)} (finishReason={cand.get('finishReason')})")
    data = base64.b64decode(blobs[0]["data"])
    if data[:4] != b"RIFF":
        die(f"audio part is not a WAV (mimeType {blobs[0].get('mimeType')}, head {data[:4]!r})")
    return data, cand.get("finishReason")


def probe_dur(path):
    out = subprocess.run(["ffprobe", "-v", "error", "-show_entries", "format=duration",
                          "-of", "default=nw=1:nk=1", path], capture_output=True, text=True, check=True)
    return float(out.stdout.strip())


def silences(path, noise_db, min_d):
    r = subprocess.run(["ffmpeg", "-hide_banner", "-nostats", "-i", path, "-af",
                        f"silencedetect=noise={noise_db}dB:d={min_d}", "-f", "null", "-"],
                       capture_output=True, text=True)
    if r.returncode != 0:
        die(f"silencedetect failed on {path}: {r.stderr[-300:]}")
    st, out = None, []
    for line in r.stderr.splitlines():
        m = re.search(r"silence_start: ([0-9.]+)", line)
        if m:
            st = float(m.group(1))
        m = re.search(r"silence_end: ([0-9.]+)", line)
        if m and st is not None:
            out.append((st, float(m.group(1))))
            st = None
    if st is not None:
        out.append((st, probe_dur(path)))
    return out


def find_bounds(wav, ids, texts, inner, amodel, key, cache, min_cut):
    """Paragraph boundaries as chosen pauses, re-aligning the remainder when the aligner drifts.

    The aligner is exact on short audio and drifts on long audio (seen 2026-10-03: from about
    two minutes on, one paragraph ended before it started). So every boundary must pass three
    checks: the aligner's times increase, the paragraph lies inside the audio, and a pause of at
    least min_cut seconds (split.min_cut, default 0.25) lies within 1.2 s of the aligner's boundary. At the first boundary that fails,
    the audio is cut at the last good pause and only the remainder is aligned again.
    """
    total = probe_dur(wav)
    bounds, start_k, offset, rounds = [], 0, 0.0, []
    while start_k < len(ids) - 1:
        key_c = f"{start_k}@{offset:.3f}"
        if key_c in cache:
            anchors = cache[key_c]
        else:
            if not key:
                die("GEMINI_API_KEY is not set (run under a login shell: zsh -ic ...)")
            part = wav + f".part{start_k}.wav"
            subprocess.run(["ffmpeg", "-y", "-hide_banner", "-loglevel", "error", "-ss", f"{offset:.3f}", "-i", wav,
                            "-c:a", "pcm_s16le", part], check=True)
            print(f"  aligning {ids[start_k]}..{ids[-1]} from {offset:.2f}s with {amodel}")
            try:
                anchors = align(part, ids[start_k:], texts[start_k:], amodel, key)
                if anchors[-1]["end"] > probe_dur(part) + 1.0:
                    raise RuntimeError(f"answer ends at {anchors[-1]['end']}s, after the audio")
            except RuntimeError as e:
                print(f"  timing answer unusable ({str(e)[:120]}); using the timed transcript")
                anchors = align_by_transcript(part, ids[start_k:], texts[start_k:], amodel, key)
            os.remove(part)
            cache[key_c] = anchors
        good = 0
        prev_end = bounds[-1][1] if bounds else 0.0
        for j in range(len(anchors) - 1):
            x, y = anchors[j], anchors[j + 1]
            if not (x["start"] <= x["end"] <= y["start"] <= y["end"]) or offset + y["end"] > total + 0.5:
                break
            anc = offset + (x["end"] + y["start"]) / 2
            cands = [p for p in inner if p[0] > prev_end + 0.3 and p[1] - p[0] >= min_cut
                     and abs((p[0] + p[1]) / 2 - anc) <= 1.2]
            if not cands:
                break
            best = min(cands, key=lambda p: abs((p[0] + p[1]) / 2 - anc))
            bounds.append((best, anc))
            prev_end = best[1]
            good += 1
        rounds.append((start_k, offset, good))
        if start_k + good >= len(ids) - 1:
            break
        if good == 0:
            die(f"{os.path.basename(wav)}: no checkable boundary after {ids[start_k]} "
                f"(aligner answer from {offset:.2f}s does not fit the pauses)")
        start_k += good
        offset = bounds[-1][0][1] - 0.05
    return bounds, rounds


def split_group(wav, ids, texts, outdir, pacing, amodel, key, cache):
    total = probe_dur(wav)
    sil = silences(wav, pacing.get("noise_db", -40), pacing.get("min_silence", 0.25))
    s0 = sil[0][1] if sil and sil[0][0] < 0.05 else 0.0
    s1 = sil[-1][0] if sil and sil[-1][1] > total - 0.05 else total
    inner = [p for p in sil if p[0] > s0 + 0.05 and p[1] < s1 - 0.05]
    found, rounds = find_bounds(wav, ids, texts, inner, amodel, key, cache, pacing.get("min_cut", 0.25))
    print(f"  split {os.path.basename(wav)}: {len(ids)} segments, speech {s0:.2f}-{s1:.2f}s, "
          f"aligner rounds {[(ids[k], round(o, 2), g) for k, o, g in rounds]}")
    worst = 0.0
    for i, (best, anc) in enumerate(found):
        mid = (best[0] + best[1]) / 2
        worst = max(worst, abs(mid - anc))
        print(f"    {ids[i]:>6}|{ids[i + 1]:<6} aligner {anc:7.2f}s  pause at {mid:7.2f}s ({mid - anc:+5.2f})  "
              f"length {best[1] - best[0]:.2f}s")
    print(f"    largest aligner-to-pause distance {worst:.2f}s")
    bounds = [b for b, _ in found]
    edges = [s0] + [x for b in bounds for x in b] + [s1]
    for k, sid in enumerate(ids):
        a, b = edges[2 * k], edges[2 * k + 1]
        a, b = max(0.0, a - 0.04), min(total, b + 0.08)
        dst = os.path.join(outdir, f"{sid}.wav")
        subprocess.run(["ffmpeg", "-y", "-hide_banner", "-loglevel", "error", "-i", wav, "-ss", f"{a:.3f}",
                        "-to", f"{b:.3f}", "-c:a", "pcm_s16le", dst + ".tmp.wav"], check=True)
        os.replace(dst + ".tmp.wav", dst)
    return worst <= 1.0


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model")
    ap.add_argument("--voice")
    ap.add_argument("--only-group", type=int)
    ap.add_argument("--outdir", default="audio")
    a = ap.parse_args()
    os.chdir(HERE)
    script = json.load(open("script.json", encoding="utf-8"))
    tts = script.get("tts")
    if not isinstance(tts, dict):
        die("script.json has no tts object")
    model = a.model or os.environ.get("CB_TTS_MODEL") or tts["model"]  # override for previews
    voice = a.voice or tts["voice"]
    segs = script["segments"]
    groups = tts["groups"]
    covered = sorted(n for g in groups for n in g)
    if covered != sorted({s["scene"] for s in segs}):
        die(f"tts.groups {groups} do not cover the scenes exactly once")
    outdir = a.outdir
    os.makedirs(os.path.join(outdir, "groups"), exist_ok=True)
    key = os.environ.get("GEMINI_API_KEY", "")
    clean = True
    for gi, g in enumerate(groups, 1):
        if a.only_group and gi != a.only_group:
            continue
        gs = [s for s in segs if s["scene"] in g]
        ids = [s["id"] for s in gs]
        texts = [s["text"] for s in gs]
        prompt = prompt_for(tts, texts)
        h = hashlib.sha256(f"{model}|{voice}|{prompt}".encode()).hexdigest()
        wav = os.path.join(outdir, "groups", f"group-{gi}.wav")
        cached = (os.path.exists(wav) and os.path.exists(wav + ".hash")
                  and open(wav + ".hash").read() == h)
        if cached:
            print(f"group {gi} (scenes {g}): cached")
        else:
            if not key:
                die("GEMINI_API_KEY is not set (run under a login shell: zsh -ic ...)")
            print(f"group {gi} (scenes {g}): calling {model}, voice {voice}, {len(ids)} segments")
            data, fr = call_tts(model, voice, prompt, key)
            open(wav + ".tmp", "wb").write(data)
            os.replace(wav + ".tmp", wav)
            open(wav + ".hash", "w").write(h)
            print(f"  received {probe_dur(wav):.2f}s audio, finishReason {fr}")
        amodel = tts.get("align_model", "gemini-3.8-flash")
        apath = wav + ".align.json"
        cache = {}
        if os.path.exists(apath):
            old = json.load(open(apath))
            if old.get("hash") == h and old.get("model") == amodel and isinstance(old.get("rounds"), dict):
                cache = old["rounds"]
        try:
            clean &= split_group(wav, ids, texts, outdir, tts.get("split", {}), amodel, key, cache)
        finally:  # keep paid aligner answers even when the split refuses
            json.dump({"hash": h, "model": amodel, "rounds": cache}, open(apath, "w"), indent=1)
    if a.only_group:
        return
    timing = {}
    for s in segs:
        p = os.path.join(outdir, f"{s['id']}.wav")
        if not os.path.exists(p):
            die(f"segment audio missing: {p}")
        timing[s["id"]] = round(probe_dur(p), 6)
    assert len(timing) == len(segs)
    tmp = "timing.json.tmp"
    json.dump(timing, open(tmp, "w"), indent=2)
    os.replace(tmp, "timing.json")
    print(f"narrate-gemini: {len(segs)} segments, narration total {sum(timing.values()):.1f}s"
          + ("" if clean else "; CHECK the split tables above"))


if __name__ == "__main__":
    main()
