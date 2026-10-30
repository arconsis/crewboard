"""transcribe_gemini.py <wav> [model] - plain transcript of a WAV from a Gemini text model (checking aid)."""
import base64, json, os, sys, urllib.request
wav = sys.argv[1]; model = sys.argv[2] if len(sys.argv) > 2 else "gemini-3.8-flash"
body = {"contents": [{"role": "user", "parts": [
    {"inlineData": {"mimeType": "audio/wav", "data": base64.b64encode(open(wav, "rb").read()).decode()}},
    {"text": "Transcribe this narration word for word. Start a new line at every pause longer than half a second, and prefix each line with its start time in seconds like [12.3]."}]}],
    "generationConfig": {"temperature": 0}}
req = urllib.request.Request(f"https://generativelanguage.googleapis.com/v1beta/models/{model}:generateContent",
                             data=json.dumps(body).encode(),
                             headers={"Content-Type": "application/json", "x-goog-api-key": os.environ["GEMINI_API_KEY"]})
d = json.load(urllib.request.urlopen(req, timeout=600))
print("".join(p.get("text", "") for p in d["candidates"][0]["content"]["parts"]))
