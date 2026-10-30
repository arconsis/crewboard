"""crewboard explainer - one manim Scene per storyboard scene (S00Title ... S13Close).

Timing rule: the video follows the voice. Every scene reads timing.json (segment
durations measured from the narration WAVs) and script.json (texts, pacing). A
scene refuses to render when one of its segment ids is missing. Per segment the
scene plays its animations and then waits until the segment's audio window is
over; the windows are cumulative, so frame rounding never drifts.

Audio layout per scene (build.sh muxes exactly this):
    lead_in silence, seg 1, gap, seg 2, gap, ..., seg N, tail silence

On-screen text rule: no token, no key, no real host or account name. Only the example
names of the docs: the owner, `claude@host-a` (coordinator), `claude@host-b` (dev agent),
`crew-bot`, `you/my-repo`, `my-crew`, ticket `#4`, PR `#5`, head `17731ed`.
`Text` only, no LaTeX.
"""

import json
import os

from manim import *  # noqa: F403

HERE = os.path.dirname(os.path.abspath(__file__))

BG = "#0e1117"
FG = "#e6e8ee"
MUTED = "#8b93a7"
DIM = "#3a4150"
PANEL = "#161b26"
BOARD = "#4c8dff"    # the board, tickets, GitHub
SIGNED = "#3ddc84"   # signatures, proof, Done
OWNER = "#ffb340"    # the owner, owner decisions
COORD = "#b07cff"    # the coordinator
DEV = "#2ec4c4"      # dev agents
REFUSAL = "#ff5a5a"  # refusals, mismatches, security events
FONT = "Helvetica"
MONO = "Menlo"

config.background_color = BG

STATUSES = ["Backlog", "Ready", "In Progress", "Review", "Blocked", "Done", "Dropped"]
ST = {s: i for i, s in enumerate(STATUSES)}
SHA = "17731ed"

# Safe area: the frame is 14.22 x 8 units.
MAX_W = 13.0


# --------------------------------------------------------------------- helpers

def T(s, size=30, color=FG, weight=NORMAL):
    return Text(s, font=FONT, font_size=size, color=color, weight=weight)


def M(s, size=20, color=FG):
    return Text(s, font=MONO, font_size=size, color=color)


def fit(m, w):
    if m.width > w:
        m.scale_to_fit_width(w)
    return m


def box(label, color=FG, w=3.0, h=1.0, size=26, fill=PANEL, text_color=None):
    r = RoundedRectangle(corner_radius=0.14, width=w, height=h, stroke_color=color,
                         stroke_width=3, fill_color=fill, fill_opacity=1)
    t = fit(T(label, size, text_color or FG), w - 0.3)
    t.move_to(r)
    return VGroup(r, t)


def pill(text, color=BOARD, size=20, pad=0.5, fill_op=0.18):
    t = T(text, size, FG)
    r = RoundedRectangle(corner_radius=0.26, width=t.width + pad, height=t.height + 0.3,
                         stroke_color=color, stroke_width=2, fill_color=color, fill_opacity=fill_op)
    t.move_to(r)
    return VGroup(r, t)


def cmd(s, size=20, color=FG, w=None, prompt="$"):
    """A command line: a muted prompt and the command in monospace."""
    g = VGroup(M(prompt, size, MUTED), M(s, size, color)).arrange(RIGHT, buff=0.18)
    if w:
        fit(g, w)
    return g


def agent_dot(label="agent", color=FG, size=20):
    d = Dot(radius=0.2, color=color)
    halo = Circle(radius=0.32, stroke_color=color, stroke_width=2, stroke_opacity=0.5)
    g = VGroup(d, halo)
    if label:
        g.add(T(label, size, MUTED).next_to(d, DOWN, buff=0.28))
    return g


def person(label, color=OWNER, scale=1.0):
    head = Circle(radius=0.2, stroke_color=color, stroke_width=3, fill_color=color, fill_opacity=0.25)
    body = Arc(radius=0.38, start_angle=0, angle=PI, stroke_color=color, stroke_width=3)
    body.set_fill(color, 0.25)
    body.next_to(head, DOWN, buff=0.08)
    g = VGroup(head, body)
    if label:
        g.add(T(label, 22, color).next_to(body, DOWN, buff=0.15))
    return g.scale(scale)


def check_mark(color=SIGNED, s=0.22):
    v = VMobject(stroke_color=color, stroke_width=5)
    v.set_points_as_corners([LEFT * s + UP * 0.05, DOWN * s * 0.8 + LEFT * s * 0.2, RIGHT * s * 1.2 + UP * s])
    return v


def x_mark(color=REFUSAL, s=0.22):
    return VGroup(Line(UL * s, DR * s, color=color, stroke_width=6),
                  Line(UR * s, DL * s, color=color, stroke_width=6))


def lock_icon(color=SIGNED, s=0.22):
    body = RoundedRectangle(corner_radius=0.04, width=2 * s, height=1.5 * s, stroke_color=color,
                            fill_color=color, fill_opacity=0.3, stroke_width=3)
    shackle = Arc(radius=s * 0.65, start_angle=0, angle=PI, stroke_color=color, stroke_width=3)
    shackle.next_to(body, UP, buff=0)
    return VGroup(body, shackle)


def key_icon(color=SIGNED, s=0.18):
    ring = Circle(radius=s, stroke_color=color, stroke_width=3)
    shaft = Line(RIGHT * s, RIGHT * s * 3.4, color=color, stroke_width=3)
    t1 = Line(RIGHT * s * 2.6, RIGHT * s * 2.6 + DOWN * s * 0.7, color=color, stroke_width=3)
    t2 = Line(RIGHT * s * 3.4, RIGHT * s * 3.4 + DOWN * s * 0.7, color=color, stroke_width=3)
    return VGroup(ring, shaft, t1, t2)


def machine(label, color=MUTED, w=2.4, h=1.5):
    screen = RoundedRectangle(corner_radius=0.08, width=w, height=h, stroke_color=color,
                              stroke_width=3, fill_color=PANEL, fill_opacity=1)
    base = Polygon([-w * 0.62, 0, 0], [w * 0.62, 0, 0], [w * 0.54, 0.16, 0], [-w * 0.54, 0.16, 0],
                   stroke_color=color, stroke_width=3, fill_color=PANEL, fill_opacity=1)
    base.next_to(screen, DOWN, buff=0.02)
    t = T(label, 20, MUTED).next_to(base, DOWN, buff=0.15)
    return VGroup(screen, base, t)


def doc_icon(title_lines, color=FG, w=2.6, h=3.2, size=20, lines=True):
    r = RoundedRectangle(corner_radius=0.1, width=w, height=h, stroke_color=color, stroke_width=3,
                         fill_color=PANEL, fill_opacity=1)
    title = VGroup(*[T(s, size, FG) for s in title_lines]).arrange(DOWN, buff=0.12)
    fit(title, w - 0.4).move_to(r.get_top() + DOWN * (0.25 + title.height / 2))
    g = VGroup(r, title)
    top = title.get_bottom()[1] - 0.25
    bottom = r.get_bottom()[1] + 0.2
    n = int((top - bottom) / 0.3) + 1 if lines else 0
    if n >= 1:
        ls = VGroup(*[Line(LEFT * (w / 2 - 0.3), RIGHT * (w / 2 - 0.3 - 0.4 * (k % 3)), color=DIM, stroke_width=3)
                      .move_to([0, top - 0.3 * k, 0]) for k in range(min(n, 5))])
        ls.move_to([r.get_center()[0], ls.get_center()[1], 0]).align_to(r.get_left() + RIGHT * 0.3, LEFT)
        g.add(ls)
    return g


def board(w=12.8, h=3.4, y=0.2, size=17):
    """The seven Status columns. `b.xs[i]` is the x of column i; `slot(b, i, k)` a card position."""
    cw = w / 7
    cols, labels, xs = VGroup(), VGroup(), []
    for i, st in enumerate(STATUSES):
        x = -w / 2 + cw * (i + 0.5)
        r = RoundedRectangle(corner_radius=0.1, width=cw - 0.14, height=h, stroke_color=DIM,
                             stroke_width=2, fill_color=PANEL, fill_opacity=1).move_to([x, y, 0])
        lab = fit(T(st, size, MUTED), cw - 0.3).move_to([x, y + h / 2 - 0.32, 0])
        cols.add(r)
        labels.add(lab)
        xs.append(x)
    g = VGroup(cols, labels)
    g.xs, g.y, g.h, g.cw = xs, y, h, cw
    return g


def slot(b, i, k=0):
    """A card position in column i, read from the column itself (correct after a shift or scale)."""
    col = b[0][i]
    return np.array([col.get_x(), col.get_top()[1] - 1.0 - 0.78 * k, 0])


def card(label="#4", color=BOARD, w=1.45, h=0.6, size=18):
    r = RoundedRectangle(corner_radius=0.1, width=w, height=h, stroke_color=color, stroke_width=3,
                         fill_color=color, fill_opacity=0.22)
    t = fit(T(label, size, FG), w - 0.2).move_to(r)
    return VGroup(r, t)


def msg(frm, to, kind, body, color=SIGNED, w=5.6, size=19):
    """A signed message: the hidden header line, the body, and the signature pill."""
    head = fit(M(f"<!-- crew v=1 from={frm} to={to} kind={kind} sig=... -->", 11, MUTED), w - 0.3)
    lines = body if isinstance(body, list) else [body]
    bt = VGroup(*[fit(T(s, size, FG), w - 0.3) for s in lines]).arrange(DOWN, buff=0.1, aligned_edge=LEFT)
    sig = pill("Ed25519 signature", color, 14, pad=0.35)
    inner = VGroup(head, bt, sig).arrange(DOWN, buff=0.18, aligned_edge=LEFT)
    r = RoundedRectangle(corner_radius=0.12, width=w, height=inner.height + 0.4, stroke_color=color,
                         stroke_width=2, fill_color=PANEL, fill_opacity=1)
    inner.move_to(r).align_to(r.get_left() + RIGHT * 0.15, LEFT)
    g = VGroup(r, inner)
    g.sig = sig
    return g


def rows_table(items, x_left, y_top, dy, w, size=22):
    """Left-aligned rows with a colored dot. items: (text, color)."""
    rows = VGroup()
    for k, (txt, col) in enumerate(items):
        d = Dot(radius=0.08, color=col)
        t = fit(T(txt, size, FG), w)
        g = VGroup(d, t).arrange(RIGHT, buff=0.25)
        g.move_to([x_left, y_top - dy * k, 0], aligned_edge=LEFT)
        rows.add(g)
    return rows


# ------------------------------------------------------------------- timing base

def _load():
    with open(os.path.join(HERE, "script.json"), encoding="utf-8") as f:
        script = json.load(f)
    tp = os.path.join(HERE, "timing.json")
    if not os.path.exists(tp):
        raise RuntimeError("timing.json is missing: run narrate.sh first")
    with open(tp, encoding="utf-8") as f:
        timing = json.load(f)
    if not isinstance(timing, dict):
        raise RuntimeError(f"timing.json is a {type(timing).__name__}, expected an object")
    return script, timing


class Narrated(Scene):
    SCENE_NO = 0

    def setup(self):
        script, timing = _load()
        self.pacing = script["pacing"]
        segs = [s for s in script["segments"] if s["scene"] == self.SCENE_NO]
        if not segs:
            raise RuntimeError(f"{type(self).__name__}: no segments for scene {self.SCENE_NO}")
        missing = [s["id"] for s in segs if s["id"] not in timing]
        if missing:
            raise RuntimeError(f"{type(self).__name__}: timing.json lacks {missing}; refusing to render")
        self.seg_ids = [s["id"] for s in segs]
        self.seg_text = {s["id"]: s["text"] for s in segs}
        self.dur = {s["id"]: float(timing[s["id"]]) for s in segs}
        self._next = 0
        self._target = float(self.pacing["lead_in"])
        self._seg_start = 0.0
        self._cur = None
        self._caption = None

    def until(self, t):
        d = t - self.time
        if d > 0.5 / config.frame_rate:
            self.wait(d)

    def seg(self, sid):
        expected = self.seg_ids[self._next] if self._next < len(self.seg_ids) else None
        if sid != expected:
            raise RuntimeError(f"segment order: got {sid}, expected {expected}")
        late = self.time - self._target
        if late > 0.05:
            print(f"TIMING overrun before {sid}: {late:.2f}s")
        self.until(self._target)
        self._seg_start = self._target
        self._cur = sid
        self._target += self.dur[sid] + float(self.pacing["gap"])
        self._next += 1

    def cue(self, phrase):
        """Wait until the narration reaches `phrase` (linear estimate over the segment's characters)."""
        text = self.seg_text[self._cur]
        idx = text.lower().find(phrase.lower())
        if idx < 0:
            raise RuntimeError(f"cue '{phrase}' not in {self._cur}")
        self.until(self._seg_start + self.dur[self._cur] * idx / len(text))

    def end_of_scene(self):
        return self._target - float(self.pacing["gap"]) + float(self.pacing["tail"])

    def finish(self):
        if self._next != len(self.seg_ids):
            raise RuntimeError(f"segments not played: {self.seg_ids[self._next:]}")
        end = self.end_of_scene()
        over = self.time - end
        if over > 0.3:
            raise RuntimeError(f"{type(self).__name__}: animations overrun the narration by {over:.2f}s")
        self.until(end)
        print(f"TIMING {type(self).__name__}: video {self.time:.3f}s, planned {end:.3f}s")

    def caption(self, s, color=MUTED, size=26, y=-3.3):
        new = fit(T(s, size, color), MAX_W).move_to([0, y, 0])
        anims = [FadeIn(new, shift=UP * 0.15)]
        if self._caption is not None:
            anims.append(FadeOut(self._caption))
        self._caption = new
        return anims

    def drop_caption(self):
        if self._caption is None:
            return []
        c, self._caption = self._caption, None
        return [FadeOut(c)]

    def heading(self, s, color=FG):
        """The scene heading, top left."""
        h = fit(T(s, 26, color), 8.0).move_to([-6.9, 3.5, 0], aligned_edge=LEFT)
        return h


# ---------------------------------------------------------------------- scenes

class S00Title(Narrated):
    """The opening card: the name and the one-line positioning of the README."""
    SCENE_NO = 0

    def construct(self):
        head = Text("crewboard", font=FONT, font_size=92, color=FG, weight=BOLD,
                    t2c={"crew": BOARD})
        head.move_to([0, 0.75, 0])
        rule = Line([-1.6, 0, 0], [1.6, 0, 0], color=SIGNED, stroke_width=4).next_to(head, DOWN, buff=0.4)
        p1 = T("A crew of agents.", 32, FG)
        p2 = T("One board.", 32, BOARD)
        p3 = T("Proof before Done.", 32, SIGNED)
        line = VGroup(p1, p2, p3).arrange(RIGHT, buff=0.4).next_to(rule, DOWN, buff=0.45)
        self.play(FadeIn(head, shift=UP * 0.15), run_time=0.7)
        self.seg("s0-1")
        self.play(Create(rule), run_time=0.4)
        self.cue("A crew")
        self.play(FadeIn(p1, shift=UP * 0.1), run_time=0.4)
        self.cue("One board")
        self.play(FadeIn(p2, shift=UP * 0.1), run_time=0.4)
        self.cue("Proof")
        self.play(FadeIn(p3, shift=UP * 0.1), run_time=0.4)
        self.until(self.end_of_scene() - 0.6)
        self.play(FadeOut(VGroup(head, rule, line)), run_time=0.5)
        self.finish()


class S01Cold(Narrated):
    SCENE_NO = 1

    def construct(self):
        hosts = VGroup()
        for k, (h, rt, col) in enumerate([("host-a", "claude", DEV), ("host-b", "codex", DEV), ("host-c", "hermes", DEV)]):
            m = machine(h).move_to([-4.8, 2.3 - 2.4 * k, 0])
            a = agent_dot(rt, col, 18).move_to(m[0].get_center() + UP * 0.15)
            hosts.add(VGroup(m, a))
        repo = box("one repository", BOARD, w=3.4, h=1.3, size=28).move_to([3.2, 0.1, 0])
        self.play(LaggedStart(*[FadeIn(h, shift=RIGHT * 0.2) for h in hosts], lag_ratio=0.3), run_time=0.9)

        self.seg("s1-1")
        self.cue("different runtimes")
        self.play(*[Indicate(h[1][0], color=DEV, scale_factor=1.3) for h in hosts], run_time=0.7)
        self.cue("one repository")
        arrows = VGroup(*[Arrow(h[0][0].get_right(), repo.get_left(), buff=0.15, color=MUTED, stroke_width=3,
                                max_tip_length_to_length_ratio=0.08) for h in hosts])
        self.play(FadeIn(repo), *[GrowArrow(a) for a in arrows], run_time=0.9)

        self.seg("s1-2")
        qs = VGroup()
        for k, (q, cue) in enumerate([("Who took this task?", "who took"), ("Who said it was finished?", "Who said"),
                                      ("What proves it?", "What proves")]):
            t = T(q, 30, OWNER if k == 2 else FG).move_to([3.2, 2.6 - 0.75 * k, 0])
            qs.add(t)
        self.play(repo.animate.shift(DOWN * 1.6), arrows.animate.set_opacity(0.35), run_time=0.6)
        for k, cue in enumerate(["who took", "Who said", "What proves"]):
            self.cue(cue)
            self.play(FadeIn(qs[k], shift=UP * 0.15), run_time=0.5)

        self.seg("s1-3")
        self.play(FadeOut(VGroup(hosts, arrows, repo)), qs.animate.scale(0.75).move_to([-3.6, 2.6, 0]), run_time=0.7)
        chat = RoundedRectangle(corner_radius=0.3, width=3.6, height=1.3, stroke_color=MUTED, stroke_width=2,
                                fill_color=PANEL, fill_opacity=1).move_to([-3.2, 0.1, 0])
        chat_t = T("chat: \"done, merged it\"", 22, MUTED).move_to(chat)
        self.play(FadeIn(VGroup(chat, chat_t)), run_time=0.5)
        self.cue("not signed")
        ns = VGroup(x_mark(REFUSAL, 0.18), T("not signed", 22, REFUSAL)).arrange(RIGHT, buff=0.2).next_to(chat, DOWN, buff=0.3)
        self.play(FadeIn(ns), run_time=0.5)
        self.cue("anyone who holds a token")
        field = box("Status: Done", BOARD, w=3.4, h=0.9, size=24).move_to([3.2, 0.1, 0])
        tok = pill("any token", MUTED, 18).next_to(field, UP, buff=0.45)
        arr = Arrow(tok.get_bottom(), field.get_top(), buff=0.08, color=MUTED, stroke_width=3)
        self.play(FadeIn(field), FadeIn(tok), GrowArrow(arr), run_time=0.6)
        wr = VGroup(x_mark(REFUSAL, 0.18), T("written by hand", 22, REFUSAL)).arrange(RIGHT, buff=0.2).next_to(field, DOWN, buff=0.3)
        self.play(FadeIn(wr), run_time=0.4)

        self.seg("s1-4")
        self.play(FadeOut(VGroup(qs, chat, chat_t, ns, field, tok, arr, wr)), run_time=0.6)
        title = T("crewboard", 64, FG, weight=BOLD).move_to([0, 1.4, 0])
        self.play(FadeIn(title, scale=1.05), run_time=0.6)
        self.cue("One binary")
        b1 = box("one binary", BOARD, w=3.4, h=1.0, size=28)
        b2 = box("one agent skill", DEV, w=3.4, h=1.0, size=28)
        pair = VGroup(b1, b2).arrange(RIGHT, buff=0.6).move_to([0, -0.3, 0])
        self.play(FadeIn(b1, shift=UP * 0.15), run_time=0.5)
        self.cue("one agent skill")
        self.play(FadeIn(b2, shift=UP * 0.15), run_time=0.5)
        self.cue("No server")
        no = T("no server, no database", 26, MUTED).move_to([0, -1.6, 0])
        self.play(FadeIn(no), run_time=0.5)
        self.finish()


class S02Idea(Narrated):
    SCENE_NO = 2

    def construct(self):
        self.seg("s2-1")
        b = board()
        self.cue("GitHub Project board")
        self.play(LaggedStart(*[FadeIn(c, shift=UP * 0.1) for c in b[0]], lag_ratio=0.08), FadeIn(b[1]), run_time=1.0)
        self.cue("A ticket is a GitHub issue")
        c4 = card("#4 issue").move_to(slot(b, ST["Ready"]))
        self.play(FadeIn(c4, scale=0.8), run_time=0.5)
        self.cue("its column is its Status")
        col = b[0][ST["Ready"]]
        self.play(col.animate.set_stroke(BOARD, 3), b[1][ST["Ready"]].animate.set_color(BOARD), run_time=0.6)

        self.seg("s2-2")
        top = VGroup(b, c4)
        self.play(top.animate.scale(0.55).move_to([0, 2.6, 0]), run_time=0.8)
        m = msg("claude@host-a", "claude@host-b", "request", ["Assigned to you: Add input validation", "ticket: you/my-repo#4"],
                SIGNED, w=7.0).move_to([0, -0.6, 0])
        self.cue("issue comment")
        self.play(FadeIn(m, shift=UP * 0.2), run_time=0.7)
        self.cue("hidden header")
        self.play(Indicate(m[1][0], color=FG, scale_factor=1.03), run_time=0.7)
        self.cue("Ed25519")
        self.play(Indicate(m.sig, color=SIGNED, scale_factor=1.15), run_time=0.7)

        self.seg("s2-3")
        self.play(m.animate.scale(0.75).move_to([-3.6, -1.0, 0]), run_time=0.7)
        rec = T("the signed log is the record", 28, SIGNED).move_to([3.2, -0.3, 0])
        view = T("the board is a view", 28, BOARD).move_to([3.2, 0.5, 0])
        self.play(FadeIn(rec, shift=LEFT * 0.2), run_time=0.5)
        self.cue("only a view")
        self.play(FadeIn(view, shift=LEFT * 0.2), run_time=0.5)
        self.cue("disagree")
        flag = VGroup(x_mark(REFUSAL, 0.16), T("mismatch: reported, the owner decides", 22, OWNER)).arrange(RIGHT, buff=0.2)
        flag.move_to([3.2, -1.4, 0])
        self.play(FadeIn(flag, shift=UP * 0.15), run_time=0.6)

        self.seg("s2-4")
        self.play(FadeOut(VGroup(m, rec, view, flag)), run_time=0.5)
        own = person("owner", OWNER).move_to([-3.6, -0.6, 0])
        co = agent_dot("coordinator", COORD, 22).move_to([0, -0.4, 0])
        d1 = agent_dot("dev agents", DEV, 22).move_to([3.3, -0.4, 0])
        d2 = agent_dot("", DEV).scale(0.7).move_to([4.2, 0.2, 0])
        self.cue("the owner")
        self.play(FadeIn(own, shift=UP * 0.15), run_time=0.5)
        self.cue("coordinator")
        self.play(FadeIn(co, shift=UP * 0.15), run_time=0.5)
        self.cue("dev agents")
        self.play(FadeIn(d1, shift=UP * 0.15), FadeIn(d2, shift=UP * 0.15), run_time=0.5)
        self.finish()


class S03Trust(Narrated):
    SCENE_NO = 3

    def construct(self):
        self.seg("s3-1")
        ag = agent_dot("claude@host-b", DEV, 24).move_to([-4.2, 1.2, 0])
        self.play(FadeIn(ag), run_time=0.5)
        self.cue("a name")
        self.play(Indicate(ag[2], color=FG, scale_factor=1.08), run_time=0.5)
        self.cue("a key")
        key = key_icon(SIGNED).next_to(ag, RIGHT, buff=0.7)
        kl = T("key", 20, MUTED).next_to(key, DOWN, buff=0.2)
        self.play(FadeIn(VGroup(key, kl), shift=LEFT * 0.15), run_time=0.5)
        self.cue("a role")
        role = pill("dev", DEV, 20).next_to(key, RIGHT, buff=0.9)
        rl = T("role", 20, MUTED).next_to(role, DOWN, buff=0.2)
        self.play(FadeIn(VGroup(role, rl), shift=LEFT * 0.15), run_time=0.5)

        self.seg("s3-2")
        acct = pill("crew account: crew-bot", BOARD, 22).move_to([3.4, 1.2, 0])
        self.cue("crew account")
        self.play(FadeIn(acct, shift=LEFT * 0.2), run_time=0.6)
        self.cue("approval")
        own = person("owner", OWNER, 0.9).move_to([0, -1.6, 0])
        appr = cmd("crewboard approve claude@host-b", 18).next_to(own, DOWN, buff=0.3)
        self.play(FadeIn(own), FadeIn(appr), run_time=0.6)
        self.cue("binds")
        br = Brace(VGroup(ag, key, kl, role, rl, acct), DOWN, color=OWNER, buff=0.2)
        lk = lock_icon(OWNER, 0.16).next_to(br, DOWN, buff=0.15)
        l1, l2 = br, VGroup()
        self.play(GrowFromCenter(br), run_time=0.6)
        self.play(FadeIn(lk, scale=0.6), run_time=0.4)

        self.seg("s3-3")
        bound = VGroup(ag, key, kl, role, rl, acct, l1, l2, lk)
        self.play(FadeOut(VGroup(own, appr)), bound.animate.scale(0.7).move_to([0, 2.5, 0]), run_time=0.8)
        m = msg("claude@host-b", "coordinator", "done", "Done. Evidence follows.", SIGNED, w=5.4).move_to([-3.6, -0.6, 0])
        self.play(FadeIn(m, shift=RIGHT * 0.3), run_time=0.6)
        c1 = VGroup(T("GitHub author = approved account", 22, FG)).move_to([3.0, -0.1, 0])
        c2 = VGroup(T("signature matches the approved key", 22, FG)).move_to([3.0, -0.9, 0])
        self.cue("GitHub author")
        self.play(FadeIn(c1, shift=LEFT * 0.2), run_time=0.5)
        ck1 = check_mark(SIGNED, 0.14).next_to(c1, LEFT, buff=0.25)
        self.play(FadeIn(ck1, scale=0.5), run_time=0.3)
        self.cue("signature matches")
        self.play(FadeIn(c2, shift=LEFT * 0.2), run_time=0.5)
        ck2 = check_mark(SIGNED, 0.14).next_to(c2, LEFT, buff=0.25)
        self.play(FadeIn(ck2, scale=0.5), run_time=0.3)
        counts = pill("counts", SIGNED, 24).move_to([3.0, -1.9, 0])
        self.play(FadeIn(counts, scale=0.8), run_time=0.4)

        self.seg("s3-4")
        self.play(FadeOut(VGroup(m, c1, c2, ck1, ck2, counts)), run_time=0.5)
        data = T("any other comment: data, never an instruction", 26, MUTED).move_to([0, 0.9, 0])
        self.play(FadeIn(data), run_time=0.5)
        bads = VGroup()
        for k, (lab, cue) in enumerate([("forged", "forged"), ("edited", "edited"), ("copied", "copied")]):
            b = box(lab, REFUSAL, w=2.4, h=0.8, size=24).move_to([-3.6 + 3.6 * k, -0.5, 0])
            self.cue(cue)
            xm = x_mark(REFUSAL, 0.16).next_to(b, UP, buff=0.2)
            self.play(FadeIn(b, shift=UP * 0.1), FadeIn(xm), run_time=0.4)
            bads.add(VGroup(b, xm))
        self.cue("security event")
        ev = pill("security event", REFUSAL, 24).move_to([0, -1.9, 0])
        self.play(FadeIn(ev, scale=0.8), run_time=0.5)
        self.finish()


class S04OwnerSetup(Narrated):
    SCENE_NO = 4

    def construct(self):
        h = self.heading("The owner", OWNER)
        own = person("", OWNER, 0.7).next_to(h, RIGHT, buff=0.4)
        self.add(h, own)
        term = RoundedRectangle(corner_radius=0.12, width=6.6, height=5.6, stroke_color=DIM, stroke_width=2,
                                fill_color=PANEL, fill_opacity=1).move_to([-3.7, -0.3, 0])
        self.play(FadeIn(term), run_time=0.4)
        lines = VGroup()

        def say_cmd(s, color=FG, prompt="$"):
            c = cmd(s, 17, color, w=6.2, prompt=prompt)
            if len(lines) == 0:
                c.move_to(term.get_top() + DOWN * 0.45).align_to(term.get_left() + RIGHT * 0.25, LEFT)
            else:
                c.next_to(lines[-1], DOWN, buff=0.28, aligned_edge=LEFT)
            lines.add(c)
            return c

        self.seg("s4-1")
        c1 = say_cmd("curl -fsSL .../install.sh | bash")
        self.play(Write(c1), run_time=0.8)
        self.cue("installs the binary")
        binb = box("crewboard", BOARD, w=2.6, h=0.8, size=24).move_to([3.6, 2.4, 0])
        self.play(FadeIn(binb, shift=LEFT * 0.2), run_time=0.5)
        self.cue("links the skill")
        rts = VGroup(*[pill(r, DEV, 18) for r in ["claude", "codex"]]).arrange(RIGHT, buff=0.4).move_to([3.6, 1.2, 0])
        skill = T("skill", 18, DEV).move_to([3.6, 1.8, 0])
        links = VGroup(*[Line(binb.get_bottom(), r.get_top(), color=DEV, stroke_width=2) for r in rts])
        self.play(Create(links), FadeIn(skill), FadeIn(rts), run_time=0.7)

        self.seg("s4-2")
        c2 = say_cmd("crewboard init --initiative my-crew \\")
        c2b = say_cmd("--repo you/my-repo --with-ci", prompt=" ")
        self.play(Write(c2), Write(c2b), run_time=0.9)
        right = VGroup(binb, skill, rts, links)
        self.play(FadeOut(right), run_time=0.4)
        self.cue("makes the board")
        mb = board(w=5.2, h=1.4, y=1.7, size=10).shift(RIGHT * 3.6)
        self.play(FadeIn(mb), run_time=0.6)
        self.cue("registry issue")
        reg = box("registry issue (pinned)", BOARD, w=3.6, h=0.7, size=19).move_to([3.6, 0.4, 0])
        self.play(FadeIn(reg, shift=UP * 0.1), run_time=0.5)
        self.cue("pull request")
        pr = doc_icon(["AUTHORITY.md"], OWNER, w=2.6, h=1.9, size=18).move_to([1.5, -1.6, 0])
        prl = T("init pull request", 16, MUTED).next_to(pr, DOWN, buff=0.12)
        self.play(FadeIn(pr, shift=UP * 0.15), FadeIn(prl), run_time=0.5)
        self.cue("only the owner merges")
        key = M("coordinator_may_merge: false", 14, OWNER).next_to(pr, RIGHT, buff=0.3)
        fit(key, 3.0)
        self.play(FadeIn(key), run_time=0.5)

        self.seg("s4-3")
        self.cue("merges it")
        merged = VGroup(check_mark(OWNER, 0.14), T("merged by the owner", 16, OWNER)).arrange(RIGHT, buff=0.15)
        merged.next_to(key, DOWN, buff=0.2).align_to(key, LEFT)
        self.play(FadeIn(merged), run_time=0.5)
        self.cue("agent add")
        c3 = say_cmd("crewboard agent add you/my-repo \\", MUTED, prompt="host-b $")
        c3b = say_cmd("--agent claude@host-b --wait-approval", MUTED, prompt="        ")
        self.play(Write(c3), Write(c3b), run_time=0.9)
        self.cue("signed join")
        jm = msg("claude@host-b", "owner", "request", "join: claude@host-b, dev", DEV, w=3.2, size=15).scale(0.8)
        jm.move_to(c3.get_right() + RIGHT * 0.3)
        self.play(FadeIn(jm), run_time=0.3)
        self.play(jm.animate.move_to(reg.get_center() + DOWN * 0.0).scale(0.5), run_time=0.9)
        self.play(FadeOut(jm), Indicate(reg, color=DEV, scale_factor=1.05), run_time=0.5)
        self.cue("waits")
        waiting = T("waits for the approval", 15, MUTED).next_to(c3b, RIGHT, buff=0.2)
        self.play(FadeIn(waiting), run_time=0.4)

        self.seg("s4-4")
        c4 = say_cmd("crewboard approve claude@host-b", OWNER)
        self.play(Write(c4), run_time=0.8)
        self.cue("prints the account")
        out = M("approved as dev; login=crew-bot", 16, MUTED)
        out.next_to(c4, DOWN, buff=0.22).align_to(c4, LEFT)
        fit(out, 6.2)
        self.play(FadeIn(out), run_time=0.5)
        self.cue("binds it")
        bind = VGroup(lock_icon(OWNER, 0.14), T("key + account bound to claude@host-b", 18, OWNER)).arrange(RIGHT, buff=0.2)
        bind.next_to(out, DOWN, buff=0.3).align_to(out, LEFT)
        fit(bind, 6.2)
        self.play(FadeIn(bind, shift=UP * 0.1), run_time=0.5)
        self.cue("no authority")
        self.play(FadeOut(waiting), FadeIn(T("active", 15, SIGNED).move_to(waiting)), run_time=0.4)
        self.finish()


class S05OwnerDay(Narrated):
    SCENE_NO = 5

    def construct(self):
        h = self.heading("The owner", OWNER)
        own = person("", OWNER, 0.7).next_to(h, RIGHT, buff=0.4)
        self.add(h, own)

        self.seg("s5-1")
        tabs = VGroup()
        for k, name in enumerate(["Board", "By Agent", "Crew", "Needs Owner"]):
            col = OWNER if name == "Needs Owner" else BOARD
            tabs.add(pill(name, col, 22))
        tabs.arrange(RIGHT, buff=0.35).move_to([0, 2.4, 0])
        b = board(h=3.2, y=-0.4)
        self.play(FadeIn(b), run_time=0.6)
        for k, cue in enumerate(["Board", "By Agent", "Crew", "Needs Owner"]):
            self.cue(cue)
            self.play(FadeIn(tabs[k], shift=UP * 0.1), run_time=0.35)

        self.seg("s5-2")
        c4 = card("#4").move_to(slot(b, ST["Blocked"]))
        self.play(FadeIn(c4, scale=0.8), run_time=0.4)
        self.cue("needs-owner")
        lab = pill("needs-owner", OWNER, 13, pad=0.3).next_to(c4, DOWN, buff=0.12)
        self.play(FadeIn(lab, shift=UP * 0.1), Indicate(tabs[3], color=OWNER, scale_factor=1.08), run_time=0.6)
        self.cue("comment on the ticket")
        ans = box("owner: \"Use the existing helper.\"", OWNER, w=4.4, h=0.7, size=18).move_to([4.2, -2.7, 0])
        self.play(FadeIn(ans, shift=UP * 0.1), run_time=0.5)
        ok = VGroup(check_mark(OWNER, 0.12), T("counts", 16, OWNER)).arrange(RIGHT, buff=0.12).next_to(ans, LEFT, buff=0.2)
        self.play(FadeIn(ok), run_time=0.3)
        self.cue("chat answer")
        chat = T("a chat answer: not signed, does not count", 20, REFUSAL).move_to([-3.6, -2.7, 0])
        self.play(FadeIn(chat), run_time=0.5)

        self.seg("s5-3")
        self.play(FadeOut(VGroup(b, c4, lab, ans, ok, chat, tabs)), run_time=0.6)
        rows = [("approve agents", "Approve agents"),
                ("merge protected changes, and by default every pull request", "Merge protected"),
                ("move <n> <status> --why: the move is in the signed log", "move a ticket")]
        rg = VGroup()
        for k, (txt, cue) in enumerate(rows):
            self.cue(cue)
            r = VGroup(T(str(k + 1), 26, OWNER, weight=BOLD), fit(T(txt, 26, FG), 10.5)).arrange(RIGHT, buff=0.4)
            r.move_to([-6.3, 1.9 - 1.0 * k, 0], aligned_edge=LEFT)
            self.play(FadeIn(r, shift=UP * 0.15), run_time=0.5)
            rg.add(r)
        self.cue("A dragged card")
        b2 = board(w=9.0, h=1.5, y=-1.9, size=11).shift(LEFT * 1.6)
        cd = card("#4", BOARD, w=1.0, h=0.42, size=14).move_to(slot(b2, ST["Review"]) + UP * 0.35)
        self.play(FadeIn(b2), FadeIn(cd), run_time=0.5)
        hand = T("dragged by hand", 15, MUTED).next_to(b2, UP, buff=0.12)
        self.play(FadeIn(hand), cd.animate.move_to(slot(b2, ST["Done"]) + UP * 0.35), run_time=0.7)
        self.cue("mismatch")
        flag = VGroup(x_mark(REFUSAL, 0.14), fit(T("status: board against signed log", 18, REFUSAL), 3.6)).arrange(RIGHT, buff=0.15)
        flag.move_to([5.0, -1.9, 0])
        self.play(FadeIn(flag, shift=LEFT * 0.15), cd[0].animate.set_stroke(REFUSAL), run_time=0.5)
        self.finish()


class S06CoordTickets(Narrated):
    SCENE_NO = 6

    def construct(self):
        h = self.heading("The coordinator", COORD)
        co = agent_dot("", COORD).scale(0.6).next_to(h, RIGHT, buff=0.4)
        self.add(h, co)

        self.seg("s6-1")
        big = agent_dot("claude@host-a", COORD, 24).move_to([-4.2, 0.6, 0])
        self.play(FadeIn(big), run_time=0.5)
        self.cue("no code")
        nos = VGroup()
        for k, s in enumerate(["no code", "no tickets of its own"]):
            g = VGroup(x_mark(REFUSAL, 0.14), T(s, 22, MUTED)).arrange(RIGHT, buff=0.2).move_to([-4.2, -0.9 - 0.6 * k, 0])
            nos.add(g)
        self.play(FadeIn(nos[0]), run_time=0.4)
        self.cue("takes no tickets")
        self.play(FadeIn(nos[1]), run_time=0.4)
        verbs = VGroup(*[pill(v, COORD, 22) for v in ["ticket new", "assign", "verify", "merge", "close"]])
        verbs.arrange(RIGHT, buff=0.3).move_to([2.6, 0.6, 0])
        fit(verbs, 7.6)
        self.cue("makes and assigns")
        self.play(LaggedStart(*[FadeIn(v, shift=UP * 0.1) for v in verbs[:2]], lag_ratio=0.3), run_time=0.6)
        self.cue("verifies deliveries")
        self.play(LaggedStart(*[FadeIn(v, shift=UP * 0.1) for v in verbs[2:]], lag_ratio=0.3), run_time=0.6)

        self.seg("s6-2")
        self.play(FadeOut(VGroup(big, nos)), verbs.animate.scale(0.7).move_to([0, 3.4, 0]).shift(RIGHT * 2.6), run_time=0.6)
        tk = RoundedRectangle(corner_radius=0.12, width=11.5, height=3.6, stroke_color=BOARD, stroke_width=3,
                              fill_color=PANEL, fill_opacity=1).move_to([0, 0.0, 0])
        tt = T("#4  Add a section \"Running the tests\" to CONTRIBUTING.md", 22, FG)
        fit(tt, 11.0).move_to(tk.get_top() + DOWN * 0.4)
        rows = [("Goal:", "add the section; it names the test command", FG),
                ("Gate:", "", SIGNED),
                ("Scope:", "CONTRIBUTING.md only", FG)]
        rg = VGroup()
        for k, (lab, txt, col) in enumerate(rows):
            l = T(lab, 20, col, weight=BOLD)
            if lab == "Gate:":
                t = VGroup(T("the CI run of the PR head SHA is green, and", 20, FG),
                           fit(M("grep -q '^## Running the tests' CONTRIBUTING.md exits 0", 17, FG), 9.6)
                           ).arrange(DOWN, buff=0.12, aligned_edge=LEFT)
            else:
                t = fit(T(txt, 20, FG), 9.6)
            g = VGroup(l, t).arrange(RIGHT, buff=0.3, aligned_edge=UP)
            g.move_to([-5.5, [0.75, -0.05, -1.0][k], 0], aligned_edge=LEFT)
            rg.add(g)
        self.play(FadeIn(tk), FadeIn(tt), run_time=0.5)
        for g, cue in zip(rg, ["a Goal", "a Gate", "a Scope"]):
            self.cue(cue)
            self.play(FadeIn(g, shift=RIGHT * 0.15), run_time=0.4)
        self.cue("must be able to fail")
        frame = SurroundingRectangle(rg[1], color=SIGNED, buff=0.2, corner_radius=0.08)
        note = T("one line; it must be able to fail", 20, SIGNED).next_to(tk, DOWN, buff=0.25)
        self.play(Create(frame), FadeIn(note), run_time=0.6)
        self.cue("grep finds")
        self.play(Indicate(rg[1][1], color=SIGNED, scale_factor=1.02), run_time=0.6)

        self.seg("s6-3")
        self.play(FadeOut(VGroup(tk, tt, rg, frame, note)), run_time=0.5)
        c = cmd("crewboard assign 4 --to claude@host-b --order 1", 20).move_to([0, 2.4, 0])
        self.play(Write(c), run_time=0.8)
        q = RoundedRectangle(corner_radius=0.12, width=3.4, height=3.4, stroke_color=DEV, stroke_width=2,
                             fill_color=PANEL, fill_opacity=1).move_to([-3.8, -0.6, 0])
        ql = T("queue of claude@host-b", 17, DEV).move_to(q.get_top() + DOWN * 0.3)
        q7 = card("#7  Order 2", DEV, w=2.8, h=0.55, size=16).move_to(q.get_center() + DOWN * 0.1)
        q9 = card("#9  Order 3", DEV, w=2.8, h=0.55, size=16).move_to(q.get_center() + DOWN * 0.9)
        self.cue("queue of one dev agent")
        self.play(FadeIn(q), FadeIn(ql), FadeIn(q7), FadeIn(q9), run_time=0.6)
        c4 = card("#4  Order 1", BOARD, w=2.8, h=0.55, size=16).move_to([0, -0.6, 0])
        self.cue("lowest first")
        self.play(FadeIn(c4), run_time=0.3)
        self.play(c4.animate.move_to(q.get_center() + UP * 0.7), run_time=0.7)
        self.cue("moves to Ready")
        st = VGroup(pill("Backlog", MUTED, 18), Arrow(ORIGIN, RIGHT * 1.0, buff=0, color=BOARD, stroke_width=3),
                    pill("Ready", BOARD, 18)).arrange(RIGHT, buff=0.2).move_to([3.4, 0.4, 0])
        self.play(FadeIn(st), run_time=0.5)
        self.cue("signed request")
        m = msg("claude@host-a", "claude@host-b", "request", "Assigned to you: #4", SIGNED, w=4.2, size=16).scale(0.85)
        m.move_to([3.4, -1.6, 0])
        self.play(FadeIn(m, shift=UP * 0.15), run_time=0.5)
        self.play(m.animate.shift(LEFT * 2.6).scale(0.6).set_opacity(0.5), run_time=0.7)

        self.seg("s6-4")
        self.play(FadeOut(VGroup(c, q, ql, q7, q9, c4, st, m)), run_time=0.5)
        ring = Circle(radius=1.4, stroke_color=COORD, stroke_width=3).move_to([-3.4, -0.4, 0])
        rl = T("watch", 26, COORD).move_to(ring.get_center() + UP * 0.2)
        rl2 = T("in the background", 16, MUTED).next_to(rl, DOWN, buff=0.12)
        self.cue("does not poll")
        self.play(Create(ring), FadeIn(rl), FadeIn(rl2), run_time=0.7)
        self.cue("wakes it")
        evs = VGroup(*[pill(e, BOARD, 16) for e in ["done on #4", "ci-finished", "question on #9"]])
        evs.arrange(DOWN, buff=0.2).move_to([-3.4, -0.4, 0]).shift(RIGHT * 4.6)
        self.play(LaggedStart(*[FadeIn(e, shift=LEFT * 0.4) for e in evs], lag_ratio=0.3), run_time=0.9)
        self.cue("Review before Backlog")
        order = VGroup(pill("1  Review: verify, merge", COORD, 20), pill("2  Backlog: assign", MUTED, 20))
        order.arrange(DOWN, buff=0.25, aligned_edge=LEFT).move_to([3.6, -0.4, 0])
        self.play(FadeOut(evs), FadeIn(order, shift=UP * 0.15), run_time=0.6)
        self.finish()


class S07Verify(Narrated):
    SCENE_NO = 7

    def construct(self):
        h = self.heading("The coordinator", COORD)
        co = agent_dot("", COORD).scale(0.6).next_to(h, RIGHT, buff=0.4)
        self.add(h, co)
        b = board(w=12.8, h=1.5, y=2.2, size=14)
        c4 = card("#4", BOARD, w=1.2, h=0.5, size=16).move_to(slot(b, ST["Review"]) + UP * 0.3)
        self.add(b, c4)

        self.seg("s7-1")
        self.cue("in Review")
        self.play(Indicate(c4, color=BOARD, scale_factor=1.15), run_time=0.5)
        self.cue("runs verify")
        c = cmd("crewboard verify 4 --body \"grep ok, log read\"", 19).move_to([-3.4, 0.6, 0])
        self.play(Write(c), run_time=0.8)
        ev = [("pull request you/my-repo#5", "pull request"), (f"head SHA {SHA}", "head SHA"),
              ("CI run for that SHA: green", "green CI run")]
        rows = VGroup()
        for k, (txt, cue) in enumerate(ev):
            self.cue(cue)
            r = VGroup(check_mark(SIGNED, 0.13), T(txt, 21, FG)).arrange(RIGHT, buff=0.25)
            r.move_to([-6.2, -0.3 - 0.65 * k, 0], aligned_edge=LEFT)
            self.play(FadeIn(r, shift=RIGHT * 0.15), run_time=0.45)
            rows.add(r)
        lab = T("the program checks the evidence", 17, MUTED).next_to(rows, DOWN, buff=0.3).align_to(rows, LEFT)
        self.play(FadeIn(lab), run_time=0.3)

        self.seg("s7-2")
        gate = VGroup(T("Gate:", 22, SIGNED, weight=BOLD),
                      T("CI green, and grep finds the heading", 20, FG)).arrange(RIGHT, buff=0.2)
        fit(gate, 5.6).move_to([3.6, 0.3, 0])
        self.play(FadeIn(gate, shift=LEFT * 0.15), run_time=0.5)
        self.cue("reads the run log")
        log = doc_icon(["run log"], MUTED, w=1.8, h=1.5, size=16).move_to([2.2, -1.3, 0])
        eye = agent_dot("the model", COORD, 15).scale(0.8).move_to([4.8, -1.3, 0])
        look = Arrow(eye[0].get_left(), log.get_right(), buff=0.1, color=COORD, stroke_width=2)
        self.play(FadeIn(log), FadeIn(eye), GrowArrow(look), run_time=0.6)
        self.cue("exactly CI")
        only = fit(T("only  Gate: ci  is checked by the program alone", 17, MUTED), 5.4).move_to([3.6, -2.5, 0])
        self.play(FadeIn(only), run_time=0.5)

        self.seg("s7-3")
        self.play(FadeOut(VGroup(gate, log, eye, look, only)), run_time=0.4)
        cf = cmd("crewboard verify 4 --fail --body \"heading missing\"", 18, REFUSAL).move_to([3.6, 0.6, 0])
        fit(cf, 6.4)
        self.play(Write(cf), run_time=0.8)
        self.cue("back to In Progress")
        self.play(c4.animate.move_to(slot(b, ST["In Progress"]) + UP * 0.3), run_time=0.8)
        self.cue("keeps its agent")
        ag = T("agent: claude@host-b", 13, DEV).next_to(c4, DOWN, buff=0.08)
        self.play(FadeIn(ag), run_time=0.3)
        self.cue("signed fail report")
        fm = msg("claude@host-a", "claude@host-b", "report", "verify: fail  heading missing", REFUSAL, w=4.6, size=16).scale(0.85)
        fm.move_to([3.6, -1.4, 0])
        self.play(FadeIn(fm, shift=UP * 0.15), run_time=0.5)

        self.seg("s7-4")
        self.play(FadeOut(VGroup(cf, fm, ag)), c4.animate.move_to(slot(b, ST["Review"]) + UP * 0.3), run_time=0.6)
        cm = cmd("crewboard merge 4", 20, SIGNED).move_to([3.6, 0.6, 0])
        al = T("AUTHORITY.md allows it", 16, MUTED).next_to(cm, DOWN, buff=0.15)
        self.cue("runs merge")
        self.play(Write(cm), FadeIn(al), run_time=0.7)
        self.cue("Otherwise the owner")
        own = person("the owner merges", OWNER, 0.7).move_to([2.2, -1.6, 0])
        cc = cmd("crewboard close 4", 18, SIGNED).move_to([5.0, -1.6, 0])
        self.play(FadeIn(own), run_time=0.4)
        self.cue("runs close")
        self.play(Write(cc), run_time=0.5)
        self.cue("Done")
        self.play(c4.animate.move_to(slot(b, ST["Done"]) + UP * 0.3), run_time=0.6)
        self.play(c4[0].animate.set_stroke(SIGNED).set_fill(SIGNED, 0.3), run_time=0.3)
        self.finish()


class S08Dev(Narrated):
    SCENE_NO = 8

    def construct(self):
        h = self.heading("The dev agent", DEV)
        dv = agent_dot("", DEV).scale(0.6).next_to(h, RIGHT, buff=0.4)
        self.add(h, dv)
        b = board(w=12.8, h=1.5, y=2.2, size=14)
        c4 = card("#4", BOARD, w=1.2, h=0.5, size=16).move_to(slot(b, ST["Ready"]) + UP * 0.3)
        self.add(b, c4)

        self.seg("s8-1")
        trio = VGroup(pill("one ticket", BOARD, 22), pill("one branch", DEV, 22), pill("one pull request", DEV, 22))
        trio.arrange(RIGHT, buff=0.5).move_to([0, 0.3, 0])
        for k, cue in enumerate(["one ticket", "one branch", "one pull request"]):
            self.cue(cue)
            self.play(FadeIn(trio[k], shift=UP * 0.15), run_time=0.4)

        self.seg("s8-2")
        self.play(FadeOut(trio), run_time=0.4)
        term = RoundedRectangle(corner_radius=0.12, width=6.4, height=4.2, stroke_color=DIM, stroke_width=2,
                                fill_color=PANEL, fill_opacity=1).move_to([-3.8, -1.3, 0])
        self.play(FadeIn(term), run_time=0.3)
        lines = VGroup()

        def tl(s, color=FG, mono=True):
            t = M(s, 15, color) if mono else T(s, 15, color)
            fit(t, 6.0)
            if len(lines) == 0:
                t.move_to(term.get_top() + DOWN * 0.4).align_to(term.get_left() + RIGHT * 0.2, LEFT)
            else:
                t.next_to(lines[-1], DOWN, buff=0.2, aligned_edge=LEFT)
            lines.add(t)
            return t

        self.cue("runs mine")
        a = tl("$ crewboard mine")
        self.play(Write(a), run_time=0.5)
        o1 = tl("Ready        #4  Add a section ...   next", MUTED)
        self.play(FadeIn(o1), run_time=0.3)
        self.cue("with take")
        a2 = tl("$ crewboard take 4")
        self.play(Write(a2), run_time=0.5)
        self.cue("In Progress")
        self.play(c4.animate.move_to(slot(b, ST["In Progress"]) + UP * 0.3), run_time=0.7)
        self.cue("branch name")
        o2 = tl("branch: crew/claude@host-b/4-add-a-section", DEV)
        self.play(FadeIn(o2), run_time=0.4)

        self.seg("s8-3")
        a3 = tl("$ crewboard mine 4")
        self.play(Write(a3), run_time=0.5)
        th = RoundedRectangle(corner_radius=0.12, width=5.6, height=3.6, stroke_color=SIGNED, stroke_width=2,
                              fill_color=PANEL, fill_opacity=1).move_to([3.5, -1.0, 0])
        thl = T("the signed thread since the assignment, newest first", 14, MUTED)
        fit(thl, 5.2).move_to(th.get_top() + DOWN * 0.3)
        items = [("owner        answer   use the existing helper", OWNER),
                 ("claude@host-a report   verify: fail, heading missing", REFUSAL),
                 ("claude@host-b report   pushed 17731ed, CI running", DEV),
                 ("claude@host-a request  Assigned to you: #4", COORD)]
        its = VGroup()
        for k, (s, col) in enumerate(items):
            t = fit(M(s, 13, col), 5.2)
            t.move_to(th.get_top() + DOWN * (0.85 + 0.55 * k)).align_to(th.get_left() + RIGHT * 0.2, LEFT)
            its.add(t)
        self.cue("signed messages")
        self.play(FadeIn(th), FadeIn(thl), run_time=0.4)
        self.play(LaggedStart(*[FadeIn(t, shift=DOWN * 0.1) for t in its], lag_ratio=0.25), run_time=1.0)
        self.cue("does not ask again")
        na = VGroup(check_mark(SIGNED, 0.12), T("answered already: no new question", 15, SIGNED)).arrange(RIGHT, buff=0.15)
        na.next_to(its, DOWN, buff=0.3).align_to(its, LEFT)
        self.play(FadeIn(na), run_time=0.4)

        self.seg("s8-4")
        self.play(FadeOut(VGroup(th, thl, its, na)), run_time=0.4)
        pr = box("PR you/my-repo#5", DEV, w=3.4, h=0.7, size=19).move_to([3.5, 0.2, 0])
        self.play(FadeIn(pr, shift=UP * 0.1), run_time=0.4)
        self.cue("CI finished event")
        evp = pill("ci-finished: green", SIGNED, 18).move_to([6.5, -1.0, 0])
        self.play(FadeIn(evp), run_time=0.2)
        self.play(evp.animate.move_to([3.5, -1.0, 0]), run_time=0.7)
        self.cue("runs done")
        a4 = tl(f"$ crewboard done 4 --pr .../pull/5 --sha {SHA}")
        self.play(Write(a4), run_time=0.7)
        self.cue("moves to Review")
        self.play(c4.animate.move_to(slot(b, ST["Review"]) + UP * 0.3), run_time=0.7)

        self.seg("s8-5")
        self.play(FadeOut(VGroup(pr, evp)), run_time=0.3)
        self.cue("runs block")
        a5 = tl("$ crewboard block 4 --why \"tool missing\"", REFUSAL)
        self.play(Write(a5), c4.animate.move_to(slot(b, ST["Blocked"]) + UP * 0.3), run_time=0.7)
        self.cue("with ask")
        a6 = tl("$ crewboard ask 4 --body \"keep the old heading?\"", OWNER)
        lab = pill("needs-owner", OWNER, 12, pad=0.25).next_to(c4, DOWN, buff=0.08)
        self.play(Write(a6), FadeIn(lab), run_time=0.7)
        self.cue("answer arrives")
        ans = pill("owner answer on #4", OWNER, 17).move_to([3.5, -0.2, 0])
        self.play(FadeIn(ans, shift=UP * 0.15), run_time=0.5)
        self.cue("next Ready ticket")
        c7 = card("#7", BOARD, w=1.2, h=0.5, size=16).move_to(slot(b, ST["Ready"]) + UP * 0.3)
        self.play(FadeIn(c7, scale=0.8), run_time=0.3)
        self.play(c7.animate.move_to(slot(b, ST["In Progress"]) + UP * 0.3), run_time=0.6)
        self.finish()


class S09Skill(Narrated):
    SCENE_NO = 9

    def construct(self):
        h = self.heading("Through the skill", DEV)
        self.add(h)

        self.seg("s9-1")
        binb = box("crewboard binary", BOARD, w=3.4, h=0.9, size=24).move_to([-4.4, 1.6, 0])
        self.play(FadeIn(binb), run_time=0.5)
        self.cue("inside the binary")
        sk = doc_icon(["SKILL.md"], DEV, w=1.5, h=1.0, size=14).scale(0.9).move_to(binb.get_center() + DOWN * 0.05)
        self.play(FadeIn(sk, scale=0.6), run_time=0.5)
        self.cue("Skill install")
        c = cmd("crewboard skill install", 19).move_to([-4.4, 0.3, 0])
        shared = M("~/.config/agent-skills/crewboard/", 14, MUTED).move_to([-4.4, -0.4, 0])
        self.play(Write(c), FadeIn(shared), run_time=0.7)
        rts = VGroup(*[pill(r, DEV if r in ("claude", "codex") else DIM, 20) for r in ["claude", "codex", "copilot", "hermes"]])
        rts.arrange(RIGHT, buff=0.45).move_to([2.6, -0.4, 0])
        links = VGroup(*[Line(shared.get_right(), r.get_left(), color=DEV if k < 2 else DIM, stroke_width=2)
                         for k, r in enumerate(rts)])
        self.cue("each runtime that is present")
        self.play(Create(links), FadeIn(rts), run_time=0.8)
        pres = T("present on this host", 14, DEV).next_to(VGroup(rts[0], rts[1]), DOWN, buff=0.15)
        self.play(FadeIn(pres), run_time=0.3)

        self.seg("s9-2")
        sk2 = doc_icon(["SKILL.md"], DEV, w=2.8, h=2.9, size=24, lines=False).move_to([-4.6, 0.2, 0])
        self.play(FadeOut(VGroup(binb, c, shared, rts, links, pres)), ReplacementTransform(sk, sk2), run_time=0.7)
        sk = sk2
        secs = VGroup(*[T(s, 19, FG) for s in ["Start", "Your loop", "Read events", "Rules", "Exit codes"]]).arrange(DOWN, buff=0.14, aligned_edge=LEFT)
        secs.next_to(sk[1], DOWN, buff=0.3).align_to(sk.get_left() + RIGHT * 0.35, LEFT)
        self.cue("commands")
        self.play(FadeIn(secs, shift=UP * 0.05), run_time=0.5)
        self.cue("role file")
        roles = VGroup(doc_icon(["coordinator.md"], COORD, w=2.1, h=1.2, size=15),
                       doc_icon(["dev.md"], DEV, w=2.1, h=1.2, size=15)).arrange(DOWN, buff=0.3).move_to([-1.3, 0.2, 0])
        rl = VGroup(*[Arrow(sk.get_right(), r.get_left(), buff=0.1, color=MUTED, stroke_width=2,
                            max_tip_length_to_length_ratio=0.1) for r in roles])
        self.play(FadeIn(roles), *[GrowArrow(a) for a in rl], run_time=0.6)
        self.cue("The owner tells")
        bubble = RoundedRectangle(corner_radius=0.3, width=6.0, height=1.5, stroke_color=OWNER, stroke_width=2,
                                  fill_color=PANEL, fill_opacity=1).move_to([3.6, 1.4, 0])
        bt = VGroup(T("Use the crewboard skill.", 19, FG), T("You are claude@host-b, a dev agent.", 19, FG)).arrange(DOWN, buff=0.1)
        fit(bt, 5.6).move_to(bubble)
        self.play(FadeIn(VGroup(bubble, bt), shift=UP * 0.15), run_time=0.6)
        self.cue("runs the CLI itself")
        runs = VGroup(*[cmd(s, 15, DEV) for s in ["crewboard start", "crewboard mine", "crewboard take 4"]])
        runs.arrange(DOWN, buff=0.15, aligned_edge=LEFT).move_to([3.6, -0.8, 0])
        self.play(LaggedStart(*[Write(r) for r in runs], lag_ratio=0.3), run_time=1.0)

        self.seg("s9-3")
        self.play(FadeOut(VGroup(sk, secs, roles, rl, bubble, bt, runs)), run_time=0.5)
        steps = ["wake or watch", "mine", "mine <n>", "act", "signed command"]
        cols = [BOARD, DEV, DEV, FG, SIGNED]
        n = len(steps)
        center = np.array([0, -0.3, 0])
        nodes = VGroup()
        for k, (s, col) in enumerate(zip(steps, cols)):
            ang = PI / 2 - 2 * PI * k / n
            p = center + 2.3 * np.array([np.cos(ang), np.sin(ang), 0])
            nodes.add(pill(s, col, 20).move_to(p))
        arrows = VGroup()
        for k in range(n):
            a, bn = nodes[k], nodes[(k + 1) % n]
            arrows.add(Arrow(a.get_center(), bn.get_center(), buff=1.0, color=DIM, stroke_width=3,
                             max_tip_length_to_length_ratio=0.15))
        self.cue("delivers events")
        self.play(FadeIn(nodes[0]), run_time=0.4)
        quiet = T("a quiet board starts no model", 16, MUTED).move_to([-5.2, -3.0, 0])
        self.cue("quiet board")
        self.play(FadeIn(quiet), run_time=0.4)
        for k, cue in enumerate(["runs mine", "ticket number", "acts", "signed commands"], start=1):
            self.cue(cue)
            self.play(GrowArrow(arrows[k - 1]), FadeIn(nodes[k]), run_time=0.45)
        self.play(GrowArrow(arrows[n - 1]), run_time=0.4)

        self.seg("s9-4")
        loop = VGroup(nodes, arrows)
        self.play(loop.animate.scale(0.55).move_to([-4.6, -0.6, 0]), FadeOut(quiet), run_time=0.6)
        codes = [("2", "refused: the rule and the fix. Correct the command.", REFUSAL, "code two"),
                 ("3", "GitHub did not answer. Retry later.", MUTED, "Code three"),
                 ("4", "config or key. Stop and tell the owner.", OWNER, "Code four")]
        rows = VGroup()
        ex = None
        for k, (code, txt, col, cue) in enumerate(codes):
            self.cue(cue)
            r = VGroup(pill(f"exit {code}", col, 20), fit(T(txt, 20, FG), 6.0)).arrange(RIGHT, buff=0.3)
            r.move_to([-1.2, 1.0 - 1.1 * k, 0], aligned_edge=LEFT)
            self.play(FadeIn(r, shift=LEFT * 0.15), run_time=0.5)
            rows.add(r)
            if k == 0:
                self.cue("corrects itself")
                ex = fit(M("refused: unread-events. Fix: run watch --timeout 1m first", 13, MUTED), 7.2)
                ex.next_to(r, DOWN, buff=0.12).align_to(r[1], LEFT)
                self.play(FadeIn(ex), run_time=0.4)

        self.seg("s9-5")
        self.play(FadeOut(VGroup(rows, ex, loop)), run_time=0.5)
        own = person("the owner", OWNER).move_to([-4.2, 0.0, 0])
        sess = RoundedRectangle(corner_radius=0.2, width=5.6, height=2.4, stroke_color=DIM, stroke_width=2,
                                fill_color=PANEL, fill_opacity=1).move_to([0.6, 0.0, 0])
        sl = T("an agent session of the owner", 16, MUTED).move_to(sess.get_top() + DOWN * 0.3)
        cmds = VGroup(cmd("crewboard init ...", 16), cmd("crewboard agent add ...", 16), cmd("crewboard approve claude@host-b", 16, OWNER))
        cmds.arrange(DOWN, buff=0.15, aligned_edge=LEFT).move_to(sess.get_center() + DOWN * 0.2)
        self.play(FadeIn(own), FadeIn(sess), FadeIn(sl), run_time=0.5)
        self.play(LaggedStart(*[Write(c) for c in cmds], lag_ratio=0.3), run_time=0.9)
        self.cue("own GitHub login")
        gh = pill("the owner's own gh login", OWNER, 16).next_to(sess, DOWN, buff=0.25)
        self.play(FadeIn(gh), run_time=0.4)
        self.cue("decisions of a person")
        dec = VGroup(person("", OWNER, 0.8), VGroup(T("approve, merge:", 22, OWNER), T("a person decides", 22, OWNER)
                                                    ).arrange(DOWN, buff=0.1, aligned_edge=LEFT)).arrange(RIGHT, buff=0.3)
        dec.move_to([5.4, 0.0, 0])
        fit(dec, 3.0)
        self.play(FadeIn(dec, shift=LEFT * 0.15), run_time=0.5)
        self.finish()


class S10Proof(Narrated):
    SCENE_NO = 10

    def construct(self):
        self.seg("s10-1")
        head = T("Proof before Done", 40, SIGNED, weight=BOLD).move_to([0, 3.0, 0])
        self.play(FadeIn(head, shift=UP * 0.1), run_time=0.5)
        done = box("Done", SIGNED, w=2.2, h=1.0, size=32).move_to([4.8, 0.2, 0])
        done[0].set_fill(SIGNED, 0.25)
        self.cue("two ways only")
        self.play(FadeIn(done), run_time=0.5)
        p1 = VGroup(pill("verify passed", COORD, 20), pill("merged at that SHA", SIGNED, 20)).arrange(RIGHT, buff=0.3)
        p1.move_to([-2.6, 1.3, 0])
        p2 = VGroup(pill("the owner merged", OWNER, 20), pill("close", COORD, 20)).arrange(RIGHT, buff=0.3)
        p2.move_to([-2.6, -0.9, 0])
        a1 = Arrow(p1.get_right(), done.get_left() + UP * 0.2, buff=0.15, color=SIGNED, stroke_width=3)
        a2 = Arrow(p2.get_right(), done.get_left() + DOWN * 0.2, buff=0.15, color=SIGNED, stroke_width=3)
        self.cue("One:")
        self.play(FadeIn(p1[0], shift=RIGHT * 0.15), run_time=0.4)
        self.cue("merged at that SHA")
        self.play(FadeIn(p1[1], shift=RIGHT * 0.15), GrowArrow(a1), run_time=0.5)
        self.cue("Two:")
        self.play(FadeIn(p2[0], shift=RIGHT * 0.15), run_time=0.4)
        self.cue("runs close")
        self.play(FadeIn(p2[1], shift=RIGHT * 0.15), GrowArrow(a2), run_time=0.5)

        self.seg("s10-2")
        p3 = VGroup(pill("merged by the crew account", MUTED, 20), pill("no verify", REFUSAL, 20)).arrange(RIGHT, buff=0.3)
        p3.move_to([-2.6, -2.6, 0])
        a3 = Arrow(p3.get_right(), done.get_bottom() + LEFT * 0.4, buff=0.15, color=REFUSAL, stroke_width=3)
        self.cue("any other account")
        self.play(FadeIn(p3, shift=RIGHT * 0.15), GrowArrow(a3), run_time=0.6)
        self.cue("no proof")
        xm = x_mark(REFUSAL, 0.3).move_to(a3.get_center())
        np_ = T("no proof: the ticket stays open", 20, REFUSAL).next_to(p3, DOWN, buff=0.25).align_to(p3, LEFT)
        self.play(FadeIn(xm, scale=0.6), FadeIn(np_), run_time=0.5)

        self.seg("s10-3")
        self.play(FadeOut(VGroup(p1, p2, p3, a1, a2, a3, xm, np_)), done.animate.move_to([3.6, 1.0, 0]), run_time=0.6)
        drop = box("Dropped", OWNER, w=2.6, h=1.0, size=30).move_to([3.6, -1.4, 0])
        drop[0].set_fill(OWNER, 0.15)
        cw = cmd("crewboard close 4 --why \"replaced by #8\"", 18).move_to([-2.0, -1.4, 0])
        fit(cw, 5.6)
        nm = pill("no merge", MUTED, 20).next_to(cw, UP, buff=0.3)
        ad = Arrow(cw.get_right(), drop.get_left(), buff=0.15, color=OWNER, stroke_width=3)
        self.cue("without a merge")
        self.play(FadeIn(nm), run_time=0.4)
        self.cue("written reason")
        self.play(Write(cw), run_time=0.6)
        self.cue("Status Dropped")
        self.play(GrowArrow(ad), FadeIn(drop), run_time=0.5)
        note = msg("claude@host-a", "owner", "info", "dropped: replaced by #8", OWNER, w=4.0, size=17).scale(0.85)
        note.next_to(drop, DOWN, buff=0.25)
        self.play(FadeIn(note, shift=UP * 0.1), run_time=0.4)
        self.cue("final")
        lk1 = lock_icon(SIGNED, 0.16).next_to(done, RIGHT, buff=0.3)
        lk2 = lock_icon(OWNER, 0.16).next_to(drop, RIGHT, buff=0.3)
        self.play(FadeIn(lk1, scale=0.6), FadeIn(lk2, scale=0.6), run_time=0.4)
        self.cue("only the owner")
        om = VGroup(person("", OWNER, 0.6), cmd("crewboard move 4 Ready --why ...", 15, OWNER)).arrange(RIGHT, buff=0.3)
        om.move_to([-2.4, 1.0, 0])
        self.play(FadeIn(om, shift=UP * 0.1), run_time=0.5)
        self.finish()


class S11BoardLog(Narrated):
    SCENE_NO = 11

    def construct(self):
        lb = T("the board", 24, BOARD).move_to([-3.4, 3.2, 0])
        ll = T("the signed log", 24, SIGNED).move_to([3.4, 3.2, 0])
        mid = Line([0, 2.8, 0], [0, -2.6, 0], color=DIM, stroke_width=2)
        self.add(lb, ll, mid)
        brow = VGroup(T("#4", 22, FG), pill("Status: Review", BOARD, 18)).arrange(RIGHT, buff=0.4).move_to([-3.4, 1.8, 0])
        lrows = VGroup(*[fit(M(s, 13, col), 6.0) for s, col in [
            ("claude@host-a  request  Assigned to you: #4", COORD),
            ("claude@host-b  ack      Taken.", DEV),
            ("claude@host-b  done     pr ... sha 17731ed", DEV)]]).arrange(DOWN, buff=0.18, aligned_edge=LEFT)
        lrows.move_to([3.4, 1.6, 0])
        self.add(brow, lrows)

        self.seg("s11-1")
        self.cue("by hand")
        tok = pill("a crew token", MUTED, 16).move_to([-3.4, 0.6, 0])
        self.play(FadeIn(tok, shift=UP * 0.1), run_time=0.4)
        newst = pill("Status: Done", REFUSAL, 18).move_to(brow[1])
        self.play(Transform(brow[1], newst), Indicate(tok, color=REFUSAL, scale_factor=1.05), run_time=0.7)
        self.cue("detects it")
        det = T("detected, not prevented", 22, OWNER).move_to([0, -3.1, 0])
        self.play(FadeIn(det), run_time=0.5)

        self.seg("s11-2")
        c = cmd("crewboard status", 18).move_to([-6.2, -0.3, 0], aligned_edge=LEFT)
        self.play(Write(c), FadeOut(tok), run_time=0.6)
        self.cue("compares")
        cmp_ = VGroup(Arrow(brow.get_right(), lrows.get_left(), buff=0.2, color=REFUSAL, stroke_width=2),
                      Arrow(lrows.get_left(), brow.get_right(), buff=0.2, color=REFUSAL, stroke_width=2))
        self.play(GrowArrow(cmp_[0]), run_time=0.5)
        self.cue("no signed message supports")
        q = T("no signed message for Done", 15, REFUSAL).next_to(lrows, DOWN, buff=0.2).align_to(lrows, LEFT)
        out = fit(M("board against signed log: 1 mismatch", 14, REFUSAL), 5.0)
        out.next_to(c, DOWN, buff=0.2).align_to(c, LEFT)
        self.play(FadeIn(q), FadeIn(out), run_time=0.5)
        self.cue("Audit")
        au = cmd("crewboard audit --since 2h", 18).next_to(out, DOWN, buff=0.3).align_to(c, LEFT)
        au_o = fit(M("field-mismatch  #4  Status=Done", 14, REFUSAL), 5.0).next_to(au, DOWN, buff=0.2).align_to(c, LEFT)
        self.play(Write(au), FadeIn(au_o), run_time=0.6)
        self.cue("tells the owner once")
        note = pill("one signed note to the owner", OWNER, 15).next_to(q, DOWN, buff=0.3).align_to(q, LEFT)
        self.play(FadeIn(note, shift=UP * 0.1), run_time=0.4)

        self.seg("s11-3")
        self.play(FadeOut(VGroup(c, out, au, au_o, q, note, cmp_)), run_time=0.5)
        mem = T("remembered note ids: 3", 15, COORD).move_to([3.4, -0.4, 0])
        marks = VGroup(*[Dot(radius=0.08, color=COORD) for _ in lrows]).arrange(DOWN, buff=0.45).next_to(lrows, LEFT, buff=0.25)
        self.cue("remembers")
        self.play(FadeIn(marks), FadeIn(mem), run_time=0.5)
        self.cue("gone on the next read")
        self.play(FadeOut(lrows[2], shift=RIGHT * 0.5), marks[2].animate.set_color(REFUSAL), run_time=0.6)
        self.cue("deleted-message")
        ev = pill("security event: deleted-message", REFUSAL, 17).move_to([3.4, -1.3, 0])
        self.play(FadeIn(ev, shift=UP * 0.1), run_time=0.5)

        self.seg("s11-4")
        self.play(FadeOut(VGroup(mem, marks, ev)), run_time=0.4)
        own = person("", OWNER, 0.6).move_to([-5.6, -0.6, 0])
        mv = cmd("crewboard move 4 Ready --why \"no merge yet\"", 16, OWNER).next_to(own, RIGHT, buff=0.3)
        fit(mv, 5.0)
        self.cue("move command")
        self.play(FadeIn(own), Write(mv), run_time=0.7)
        self.cue("in the log")
        lrow = fit(M("owner          /crew move Ready   no merge yet", 13, OWNER), 6.0)
        lrow.move_to(lrows[2].get_center()).align_to(lrows, LEFT)
        back = pill("Status: Ready", BOARD, 18).move_to(brow[1])
        self.play(FadeIn(lrow, shift=LEFT * 0.4), Transform(brow[1], back), run_time=0.7)
        self.finish()


class S12Limits(Narrated):
    SCENE_NO = 12

    def construct(self):
        hg = T("what it gives", 30, SIGNED, weight=BOLD).move_to([-3.4, 3.0, 0])
        hb = T("what it does not do", 30, OWNER, weight=BOLD).move_to([3.4, 3.0, 0])
        sep = Line([-6.6, 2.6, 0], [6.6, 2.6, 0], color=DIM, stroke_width=2)
        mid = Line([0, 2.6, 0], [0, -3.3, 0], color=DIM, stroke_width=2)

        self.seg("s12-1")
        self.play(FadeIn(hg), Create(sep), Create(mid), run_time=0.6)
        gives = [("two or more coding agents, one GitHub repo", "Two or more"),
                 ("a shared queue", "shared queue"),
                 ("a record of who did what", "record of who"),
                 ("a check on every done", "a check on"),
                 ("no server, no database", "No server")]
        gl = VGroup()
        for k, (txt, cue) in enumerate(gives):
            self.cue(cue)
            r = VGroup(check_mark(SIGNED, 0.12), fit(T(txt, 21, FG), 5.4)).arrange(RIGHT, buff=0.25)
            r.move_to([-6.4, 2.0 - 0.8 * k, 0], aligned_edge=LEFT)
            self.play(FadeIn(r, shift=RIGHT * 0.15), run_time=0.4)
            gl.add(r)

        nots = [("s12-2", [("a Done written by hand: detected, not prevented", "written by hand"),
                           ("one coordinator: a few agents, not swarms", "One coordinator"),
                           ("a leaked token acts as its account, not as an agent", "leaked crew token")]),
                ("s12-3", [("GitHub only; agent hosts run macOS or Linux", "GitHub only"),
                           ("the setup has manual steps", "manual steps"),
                           ("no sandbox: shell commands as your user", "no sandbox"),
                           ("fully tested: interactive Claude Code on macOS only", "fully tested")]),
                ("s12-4", [("correct code: your Gate, your CI, the owner's review", "Your Gate")])]
        k = 0
        for sid, items in nots:
            self.seg(sid)
            if sid == "s12-2":
                self.play(FadeIn(hb), run_time=0.4)
            for txt, cue in items:
                self.cue(cue)
                r = VGroup(Dot(radius=0.08, color=OWNER), fit(T(txt, 20, FG), 5.6)).arrange(RIGHT, buff=0.25)
                r.move_to([0.4, 2.0 - 0.68 * k, 0], aligned_edge=LEFT)
                self.play(FadeIn(r, shift=LEFT * 0.15), run_time=0.4)
                k += 1
        self.finish()


class S13Close(Narrated):
    SCENE_NO = 13

    def construct(self):
        self.seg("s13-1")
        h1 = T("Quick try", 36, BOARD, weight=BOLD).move_to([-3.4, 2.2, 0])
        self.play(FadeIn(h1, shift=UP * 0.1), run_time=0.5)
        items = [("one person", "one person"), ("one machine", "one machine"), ("your own token", "your own token"),
                 ("a test repo", "a test repo"), ("one ticket", "one ticket")]
        qt = VGroup(*[T(txt, 24, FG) for txt, _ in items]).arrange(DOWN, buff=0.28).move_to([-3.4, 0.1, 0])
        for t, (_, cue) in zip(qt, items):
            self.cue(cue)
            self.play(FadeIn(t, shift=UP * 0.1), run_time=0.3)
        self.cue("Nine steps")
        n1 = T("9 steps, 3 terminals", 22, MUTED).move_to([-3.4, -2.2, 0])
        self.play(FadeIn(n1), run_time=0.4)

        self.seg("s13-2")
        h2 = T("A real crew", 36, SIGNED, weight=BOLD).move_to([3.4, 2.2, 0])
        self.play(FadeIn(h2, shift=UP * 0.1), run_time=0.5)
        ritems = [("a crew account", "crew account"), ("a coordinator", "a coordinator"), ("a dev agent", "a dev agent")]
        rc = VGroup(*[T(txt, 24, FG) for txt, _ in ritems]).arrange(DOWN, buff=0.28)
        rc.move_to([3.4, 0, 0]).align_to(qt, UP)
        for t, (_, cue) in zip(rc, ritems):
            self.cue(cue)
            self.play(FadeIn(t, shift=UP * 0.1), run_time=0.3)
        self.cue("thirteen steps")
        n2 = T("13 steps", 22, MUTED).move_to([3.4, -2.2, 0])
        self.play(FadeIn(n2), run_time=0.4)

        self.seg("s13-3")
        self.play(FadeOut(VGroup(h1, qt, n1, h2, rc, n2)), run_time=0.4)
        head = Text("crewboard", font=FONT, font_size=92, color=FG, weight=BOLD, t2c={"crew": BOARD}).move_to([0, 0.7, 0])
        line = VGroup(T("A crew of agents.", 30, FG), T("One board.", 30, BOARD), T("Proof before Done.", 30, SIGNED))
        line.arrange(RIGHT, buff=0.4).next_to(head, DOWN, buff=0.5)
        self.play(FadeIn(head, scale=1.05), run_time=0.5)
        self.cue("A crew")
        self.play(FadeIn(line, shift=UP * 0.1), run_time=0.5)
        self.finish()


ALL_SCENES = ["S00Title", "S01Cold", "S02Idea", "S03Trust", "S04OwnerSetup", "S05OwnerDay", "S06CoordTickets",
              "S07Verify", "S08Dev", "S09Skill", "S10Proof", "S11BoardLog", "S12Limits", "S13Close"]
