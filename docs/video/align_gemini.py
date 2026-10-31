"""align_gemini.py - ask a Gemini text model where each paragraph starts and ends in a group WAV.

Used by narrate_gemini.py as the second reader of the split: the silence detector proposes
pauses, this answer says which pause belongs to which paragraph boundary. Prints JSON.
stdlib only. The key goes only into the request header.
"""
import base64
import difflib
import re
import json
import os
import sys
import time
import urllib.error
import urllib.request

FALLBACK = ["gemini-3.8-flash", "gemini-3.5-flash", "gemini-flash-latest"]
API = "https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent"


def align(wav_path, ids, texts, model, key):
    audio = base64.b64encode(open(wav_path, "rb").read()).decode()
    paras = "\n".join(f"[{i}] {t}" for i, t in zip(ids, texts))
    ask = ("This audio is a narrator reading the paragraphs below, in order. For every paragraph, "
           "give the time in seconds (decimals allowed) where its first word starts and where its last "
           "word ends in the audio. Also say whether the paragraph was read completely. Answer only JSON: "
           'a list of objects {"id": str, "start": number, "end": number, "complete": boolean}.\n\n' + paras)
    body = {"contents": [{"role": "user", "parts": [
        {"inlineData": {"mimeType": "audio/wav", "data": audio}}, {"text": ask}]}],
        "generationConfig": {"responseMimeType": "application/json", "temperature": 0}}
    models = [model] + [m for m in FALLBACK if m != model]
    d, errs = None, []
    for m in models:
        for attempt in range(3):  # 503 "high demand" is transient; 429 is a quota and moves on
            req = urllib.request.Request(API.format(model=m), data=json.dumps(body).encode(),
                                         headers={"Content-Type": "application/json", "x-goog-api-key": key})
            try:
                with urllib.request.urlopen(req, timeout=600) as r:
                    d = json.load(r)
                break
            except urllib.error.HTTPError as e:
                errs.append(f"{m}: HTTP {e.code}")
                if e.code in (500, 503) and attempt < 2:
                    time.sleep(20 * (attempt + 1))
                    continue
                break
        if d is not None:
            break
    if d is None:
        raise RuntimeError(f"align failed on every model: {errs}")
    if not isinstance(d, dict) or not d.get("candidates"):
        raise RuntimeError(f"align: unexpected response {type(d).__name__}")
    txt = "".join(p.get("text", "") for p in d["candidates"][0].get("content", {}).get("parts", []))
    out = json.loads(txt)
    if not isinstance(out, list) or [o.get("id") for o in out] != list(ids):
        raise RuntimeError(f"align: answer does not list the ids in order: {txt[:300]}")
    return out


if __name__ == "__main__":
    wav, model = sys.argv[1], sys.argv[2]
    script = json.load(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "script.json")))
    scenes = [int(x) for x in sys.argv[3].split(",")]
    segs = [s for s in script["segments"] if s["scene"] in scenes]
    res = align(wav, [s["id"] for s in segs], [s["text"] for s in segs], model, os.environ["GEMINI_API_KEY"])
    print(json.dumps(res, indent=1))


def _seconds(stamp):
    """A transcript time stamp in seconds: `12.3`, or `mm:ss` / `mm:ss.s` (the model sometimes
    answers in clock form although the prompt asks for seconds)."""
    parts = stamp.split(":")
    secs = 0.0
    for part in parts:
        secs = secs * 60 + float(part)
    return secs


def _words(s):
    return re.sub(r"[^a-z0-9 ]", " ", s.lower()).split()


def align_by_transcript(wav_path, ids, texts, model, key):
    """Fallback reader: a timed transcript, one line per pause; each paragraph is anchored where its
    opening words appear (fuzzy, in order), interpolated inside a line when two paragraphs share one.
    Seen 2026-10-03: the JSON answer drifted past the end of a 147 s file while a timed
    transcript of the same file was exact."""
    audio = base64.b64encode(open(wav_path, "rb").read()).decode()
    ask = ("Transcribe this narration word for word. Start a new line at every pause longer than half "
           "a second, and prefix each line with its start time in seconds like [12.3].")
    body = {"contents": [{"role": "user", "parts": [
        {"inlineData": {"mimeType": "audio/wav", "data": audio}}, {"text": ask}]}],
        "generationConfig": {"temperature": 0}}
    req = urllib.request.Request(API.format(model=model), data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json", "x-goog-api-key": key})
    with urllib.request.urlopen(req, timeout=600) as r:
        d = json.load(r)
    txt = "".join(p.get("text", "") for p in d["candidates"][0].get("content", {}).get("parts", []))
    lines = [(_seconds(m.group(1)), m.group(2)) for m in re.finditer(r"\[([0-9:.]+)\]\s*(.*)", txt)]
    if not lines:
        raise RuntimeError(f"transcript has no timed lines: {txt[:200]}")
    # flat word list with an estimated time per word (linear inside a line)
    flat = []
    for k, (t0, line) in enumerate(lines):
        t1 = lines[k + 1][0] if k + 1 < len(lines) else t0 + 0.4 * max(1, len(_words(line)))
        ws = _words(line)
        for w_i, w in enumerate(ws):
            flat.append((w, t0 + (t1 - t0) * w_i / max(1, len(ws)), w_i == 0))
    starts, pos = [], 0
    for pid, t in zip(ids, texts):
        head = " ".join(_words(t)[:4])
        hit = None
        for q in range(pos, len(flat)):
            cand = " ".join(w for w, _, _ in flat[q:q + 4])
            if difflib.SequenceMatcher(None, head, cand).ratio() >= 0.75:
                hit = q
                break
        if hit is None:
            raise RuntimeError(f"transcript: no words open paragraph {pid} ('{head}')")
        starts.append(flat[hit][1])
        pos = hit + max(1, len(_words(t)) // 2)
    out = []
    for k, pid in enumerate(ids):
        end = starts[k + 1] - 0.4 if k + 1 < len(ids) else starts[k] + 1.0
        out.append({"id": pid, "start": starts[k], "end": max(starts[k], end), "complete": True,
                    "via": "transcript"})
    return out
