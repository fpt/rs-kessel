"""Generate and post-process sprite art with OpenAI's image models.

A build-time asset tool: it touches the network and the disk, and nothing in the
console depends on it. The output is raw PNGs; `spritegen.py` is what turns those
into `sprite` declarations.

The model cannot emit transparency, so art is generated on a solid magenta key
(#FF00FF) and keyed out here. Multi-object sheets — four scenery pieces, four
candidate cars — are generated as one grid image and split into cells, because
one generation gives four cells in a consistent style for the price of one, and
a set that will share a 15-colour sprite bank wants that consistency.

**Two backends, and `codex` is the default.** `--backend codex` drives the local
Codex agent through its Python SDK and lets the built-in `image_gen` tool do the
work, which bills a ChatGPT/Codex subscription and needs no API key at all.
`--backend api` posts to `/v1/images/*` with `OPENAI_API_KEY`, which is what this
tool did originally and is still the only path with real control: `--size`,
`--model` and `--quality` reach the model there and are ignored under `codex`.

Usage:
  uv run --project tools tools/imgen.py gen --out raw.png --prompt "..."
  uv run --project tools tools/imgen.py edit --out raw.png --prompt "..." --ref photo.jpg
  uv run --project tools tools/imgen.py gen --backend api --size 1024x1024 --out raw.png ...
  uv run --project tools tools/imgen.py process --in raw.png --out sprite.png
  uv run --project tools tools/imgen.py process --in sheet.png --out-dir cells \
      --rows 2 --cols 2 --labels se,sw,nw,ne

The codex backend needs the SDK, which is an extra because it drags a ~300 MB
Codex binary along: `uv sync --project tools --extra codex`. `process` and
`spritegen.py` — the free half of the pipeline, and the half you iterate on —
need neither backend.
"""

from __future__ import annotations

import argparse
import base64
import os
import shutil
import sys
from pathlib import Path

import httpx
import numpy as np
from PIL import Image

API = "https://api.openai.com/v1/images"
KEY_RGB = (255, 0, 255)
TIMEOUT = 480.0  # a high-quality 1024² generation really can take minutes

CODEX_HOME = Path(os.environ.get("CODEX_HOME") or Path.home() / ".codex")
# Codex ships `imagegen` as a system skill. Naming it explicitly beats hoping the
# agent picks it: the turn is one sentence long and has no other context to go on.
IMAGEGEN_SKILL = CODEX_HOME / "skills" / ".system" / "imagegen"

MIME = {".png": "image/png", ".webp": "image/webp"}


def fail(msg: str) -> None:
    print(f"imgen: {msg}", file=sys.stderr)
    raise SystemExit(1)


def api_key() -> str:
    key = os.environ.get("OPENAI_API_KEY", "")
    if not key:
        fail("OPENAI_API_KEY is not set")
    return key


def images(resp: httpx.Response) -> list[bytes]:
    """The PNGs out of an images response, or a readable error."""
    try:
        body = resp.json()
    except ValueError:
        fail(f"decode response (status {resp.status_code}): {resp.text[:200]}")
    if err := body.get("error"):
        fail(f"api error: {err.get('message')}")
    data = body.get("data") or []
    if not data:
        fail(f"api returned no images (status {resp.status_code})")
    return [base64.b64decode(d["b64_json"]) for d in data]


def write_all(out: Path, blobs: list[bytes]) -> None:
    """One image keeps the name; several get -0, -1, … so a retry is comparable."""
    for i, blob in enumerate(blobs):
        path = out if len(blobs) == 1 else out.with_stem(f"{out.stem}-{i}")
        path.write_bytes(blob)
        print(f"wrote {path} ({len(blob)} bytes)")


def prompt_text(args: argparse.Namespace) -> str:
    text = args.prompt or ""
    if args.prompt_file:
        text = Path(args.prompt_file).read_text()
    if not text.strip():
        fail("a --prompt or --prompt-file is required")
    return text


# ---- codex backend ---------------------------------------------------------


def codex_generate(args: argparse.Namespace, refs: list[str]) -> list[Path]:
    """Generate through the local Codex agent's built-in `image_gen` tool.

    No API key: the tool runs on whatever account `codex login` holds, so a
    ChatGPT/Codex subscription pays for this instead of API credits. The catch is
    that the built-in tool exposes no size, model or output-path control — Codex
    writes into `$CODEX_HOME/generated_images/<thread>/<call_id>.png` and the
    only way to learn that path is the thread item this reads back.
    """
    try:
        from openai_codex import (
            Codex,
            CodexConfig,
            LocalImageInput,
            Sandbox,
            SkillInput,
            TextInput,
        )
    except ModuleNotFoundError:
        fail(
            "the codex backend needs the Codex SDK: "
            "`uv sync --project tools --extra codex` "
            "(or `--backend api` to use OPENAI_API_KEY and the Images API)"
        )

    if not IMAGEGEN_SKILL.is_dir():
        fail(f"no imagegen skill at {IMAGEGEN_SKILL} — is the Codex CLI installed?")

    # Say "do not move it" out loud. The skill's own policy is to copy a
    # project-bound asset into the workspace, which for us is a race: we want the
    # original path back and then place the file ourselves.
    plural = "one image" if args.n == 1 else f"{args.n} images"
    instructions = (
        f"Use $imagegen to generate exactly {plural} from the prompt below. "
        "Use the prompt verbatim as the specification; do not add creative "
        "requirements of your own. Do not copy, move, rename or delete the "
        "generated file, and do not write anything to the workspace — report the "
        "path Codex saved it to and stop.\n\nPrompt:\n" + prompt_text(args)
    )
    items = [SkillInput(name="imagegen", path=str(IMAGEGEN_SKILL)), TextInput(instructions)]
    # A reference image has to be *in* the conversation for the built-in editor to
    # see it; a filesystem path in the prompt is not enough.
    items += [LocalImageInput(path=str(Path(r).resolve())) for r in refs]

    config = CodexConfig(codex_bin=args.codex_bin) if args.codex_bin else None
    saved: list[Path] = []
    with Codex(config=config) as codex:
        # read_only on purpose: nothing here needs to write, and the one thing
        # that does write (Codex saving the image) is not a sandboxed command.
        thread = codex.thread_start(sandbox=Sandbox.read_only)
        result = thread.run(items)
        for item in result.items:
            inner = getattr(item, "root", item)
            if getattr(inner, "type", None) != "imageGeneration":
                continue
            if getattr(inner, "status", None) not in (None, "completed"):
                print(f"imgen: an image reported status {inner.status}", file=sys.stderr)
            path = getattr(inner, "saved_path", None)
            if path is None:
                continue
            saved.append(Path(getattr(path, "root", path)))

    if not saved:
        note = result.final_response or "(the agent said nothing)"
        fail(
            "codex returned no image. The usual cause is authentication: the "
            "built-in image tool is absent under API-key auth even though "
            "`codex features list` shows image_generation enabled. Check "
            "`codex login status` — it has to say ChatGPT. The agent said: " + note
        )
    if len(saved) != args.n:
        print(f"imgen: asked for {args.n} image(s), got {len(saved)}", file=sys.stderr)
    return saved


def place(out: Path, sources: list[Path]) -> None:
    """Copy what Codex saved to where the caller asked for it.

    Copy rather than move: the originals under `$CODEX_HOME` are the only record
    of a generation, and a rerun that overwrites `--out` should not also destroy
    the previous attempt you were comparing against.
    """
    for i, src in enumerate(sources):
        path = out if len(sources) == 1 else out.with_stem(f"{out.stem}-{i}")
        shutil.copyfile(src, path)
        print(f"wrote {path} ({path.stat().st_size} bytes, from {src})")


def cmd_gen(args: argparse.Namespace) -> None:
    if args.backend == "codex":
        place(args.out, codex_generate(args, []))
        return
    resp = httpx.post(
        f"{API}/generations",
        headers={"Authorization": f"Bearer {api_key()}"},
        json={
            "model": args.model,
            "prompt": prompt_text(args),
            "size": args.size,
            "quality": args.quality,
            "n": args.n,
            "output_format": "png",
        },
        timeout=TIMEOUT,
    )
    write_all(args.out, images(resp))


def cmd_edit(args: argparse.Namespace) -> None:
    """Generate conditioned on reference images, e.g. a photo of the real thing."""
    if not args.ref:
        fail("edit: at least one --ref image is required")
    if args.backend == "codex":
        place(args.out, codex_generate(args, args.ref))
        return
    files = []
    for ref in args.ref:
        p = Path(ref)
        # The API rejects application/octet-stream, so name the real type.
        files.append(("image[]", (p.name, p.read_bytes(), MIME.get(p.suffix.lower(), "image/jpeg"))))
    resp = httpx.post(
        f"{API}/edits",
        headers={"Authorization": f"Bearer {api_key()}"},
        data={
            "model": args.model,
            "prompt": prompt_text(args),
            "size": args.size,
            "quality": args.quality,
            "n": str(args.n),
            "output_format": "png",
        },
        files=files,
        timeout=TIMEOUT,
    )
    write_all(args.out, images(resp))


# ---- chroma key ------------------------------------------------------------


def key_distance(rgb: np.ndarray) -> np.ndarray:
    return np.sqrt(((rgb - np.asarray(KEY_RGB, dtype=np.float32)) ** 2).sum(-1))


def chroma_key(img: np.ndarray, hard: float, soft: float) -> None:
    """Clear the key colour to transparency, in place.

    Two passes, and both are needed:

    * a hard global pass, so a pixel very close to the key vanishes wherever it
      is — including in a concavity the flood below cannot reach, which is where
      a halo would otherwise survive;
    * a soft flood inward from the borders, which takes the *connected*
      background and its antialiased fringe without punching holes in art that
      merely contains a similar colour.
    """
    rgb = img[:, :, :3].astype(np.float32)
    alpha = img[:, :, 3]
    dist = key_distance(rgb)

    img[(alpha > 0) & (dist < hard)] = 0

    # A pixel stops the flood if it is opaque *and* far from the key; everything
    # else is background it may spread through.
    through = ~((img[:, :, 3] != 0) & (dist >= soft))
    bg = np.zeros(through.shape, dtype=bool)
    bg[0, :] = bg[-1, :] = True
    bg[:, 0] = bg[:, -1] = True
    bg &= through

    # Grow the seeds one pixel at a time until nothing changes. A per-pixel BFS
    # would be the obvious port of the Go original and is ~1M Python loop
    # iterations; four whole-array shifts per step is the same fill in numpy.
    while True:
        grown = bg.copy()
        grown[1:, :] |= bg[:-1, :]
        grown[:-1, :] |= bg[1:, :]
        grown[:, 1:] |= bg[:, :-1]
        grown[:, :-1] |= bg[:, 1:]
        grown &= through
        if np.array_equal(grown, bg):
            break
        bg = grown

    img[bg] = 0


# ---- process ---------------------------------------------------------------


def content_bounds(img: np.ndarray) -> tuple[int, int, int, int] | None:
    ys, xs = np.nonzero(img[:, :, 3])
    if len(ys) == 0:
        return None
    return int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1


def finish(cell: np.ndarray, trim: bool, scale: int) -> Image.Image:
    """Trim transparent borders, then scale to a target width."""
    if trim and (bb := content_bounds(cell)) is not None:
        x0, y0, x1, y1 = bb
        cell = cell[y0:y1, x0:x1]
    out = Image.fromarray(cell, "RGBA")
    if scale > 0 and out.width > 0:
        h = max(1, round(out.height * scale / out.width))
        out = out.resize((scale, h), Image.LANCZOS)
    return out


def cmd_process(args: argparse.Namespace) -> None:
    src = np.asarray(Image.open(args.input).convert("RGBA")).copy()
    chroma_key(src, args.hard, args.soft)

    if args.rows * args.cols <= 1:
        out = args.out or args.input.with_stem(f"{args.input.stem}-sprite")
        img = finish(src, not args.no_trim, args.scale)
        img.save(out)
        print(f"wrote {out} ({img.width}x{img.height})")
        return

    if not args.out_dir:
        fail("process: --out-dir is required when splitting a sheet")
    args.out_dir.mkdir(parents=True, exist_ok=True)

    n = args.rows * args.cols
    labels = [s.strip() for s in args.labels.split(",")] if args.labels else []
    labels += [f"frame-{i}" for i in range(len(labels), n)]

    ch, cw = src.shape[0] // args.rows, src.shape[1] // args.cols
    for i in range(n):
        r, c = divmod(i, args.cols)
        cell = src[r * ch:(r + 1) * ch, c * cw:(c + 1) * cw]
        img = finish(cell, not args.no_trim, args.scale)
        path = args.out_dir / f"{labels[i]}.png"
        img.save(path)
        print(f"wrote {path} ({img.width}x{img.height})")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = ap.add_subparsers(dest="cmd", required=True)

    for name in ("gen", "edit"):
        p = sub.add_parser(name)
        p.add_argument("--out", required=True, type=Path)
        p.add_argument("--prompt")
        p.add_argument("--prompt-file")
        p.add_argument("--backend", default="codex", choices=("codex", "api"),
                       help="codex: the local agent's built-in tool, no API key "
                            "(default). api: /v1/images with OPENAI_API_KEY, and "
                            "the only backend where --model/--size/--quality do "
                            "anything")
        p.add_argument("--codex-bin",
                       help="a specific `codex` executable for the codex backend "
                            "(default: the one the SDK ships)")
        p.add_argument("--model", default="gpt-image-2", help="api backend only")
        p.add_argument("--size", default="1024x1024",
                       help="api backend only; gpt-image-2's minimum is 1024x1024")
        p.add_argument("--quality", default="medium", choices=("low", "medium", "high"),
                       help="api backend only")
        p.add_argument("-n", type=int, default=1,
                       help="exact on the api backend, a request on the codex one")
        if name == "edit":
            p.add_argument("--ref", action="append", default=[],
                           help="reference image (repeatable)")
        p.set_defaults(fn=cmd_gen if name == "gen" else cmd_edit)

    p = sub.add_parser("process")
    p.add_argument("--in", dest="input", required=True, type=Path)
    p.add_argument("--out", type=Path, help="single-frame mode")
    p.add_argument("--out-dir", type=Path, help="sheet mode")
    p.add_argument("--rows", type=int, default=1)
    p.add_argument("--cols", type=int, default=1)
    p.add_argument("--labels", help="comma-separated cell names, row-major")
    p.add_argument("--scale", type=int, default=0,
                   help="target width in px (0 = keep source size)")
    p.add_argument("--no-trim", action="store_true",
                   help="keep the transparent border instead of cropping to content")
    p.add_argument("--hard", type=float, default=110.0,
                   help="any pixel this close to the key is cleared, anywhere")
    p.add_argument("--soft", type=float, default=180.0,
                   help="the border flood stops at opaque pixels this far from the key")
    p.set_defaults(fn=cmd_process)

    args = ap.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
