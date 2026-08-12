#!/usr/bin/env python3
"""Generate crates/mezon-theme/src/surfaces.rs from the mezon-react theme CSS.

Reads the gradient-valued custom properties from the React theme files and emits
the static gradient table consumed by `ThemeSurfaces::for_theme`. Only tokens whose
CSS carries a real (non-flat) `linear-gradient` are emitted; flat single-colour
overlays are already represented exactly by the flattened `ThemeTokens` values.

Usage:
    scripts/gen_theme_surfaces.py [path/to/mezon/libs/assets/src/assets/themes]
"""
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
# The mezon-react checkout normally sits beside this repo; override with argv[1].
DEFAULT_THEMES_DIR = REPO_ROOT.parent / "mezon/libs/assets/src/assets/themes"
OUT = REPO_ROOT / "crates/mezon-theme/src/surfaces.rs"

# CSS custom property -> field on SurfaceGradients / ThemeSurfaces.
SURFACES = [
    ("--bg-primary", "primary"),
    ("--bg-secondary", "secondary"),
    ("--bg-surface", "surface"),
    ("--bg-theme-direct-message", "direct_message"),
    ("--bg-theme-input-primary", "input_primary"),
    ("--bg-active-friend-list", "active_friend_list"),
    ("--bg-modal-theme-search", "modal_search"),
    ("--bg-outside-footer", "outside_footer"),
    ("--bg-footer", "footer"),
]

# Themes known to ThemeTokens::for_theme; anything else falls back to solids.
THEMES = {
    "light": "light",
    "sunrise": "sunrise",
    "purple_haze": "purple_haze",
    "redDark": "redDark",
    "abyss_dark_": "abyss_dark",
    "berrynade": "berrynade",
    "cisher": "cisher",
    "sunset": "sunset",
}

FLAT_EPSILON = 0.06


def strip_comments(text):
    return re.sub(r"/\*.*?\*/", "", text, flags=re.S)


def split_top(s, sep=","):
    out, depth, cur = [], 0, ""
    for ch in s:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        if ch == sep and depth == 0:
            out.append(cur.strip())
            cur = ""
        else:
            cur += ch
    if cur.strip():
        out.append(cur.strip())
    return out


def split_space_top(s):
    out, depth, cur = [], 0, ""
    for ch in s:
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
        if ch.isspace() and depth == 0:
            if cur.strip():
                out.append(cur.strip())
            cur = ""
        else:
            cur += ch
    if cur.strip():
        out.append(cur.strip())
    return out


def declarations(text):
    text = strip_comments(text)
    body = text[text.index("{") + 1 : text.rindex("}")]
    out = {}
    for part in split_top(body, ";"):
        if ":" not in part:
            continue
        name, _, value = part.partition(":")
        name = name.strip()
        if name.startswith("--"):
            out[name] = " ".join(value.split())
    return out


def srgb_to_linear(c):
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def linear_to_srgb(c):
    if c <= 0.0031308:
        return 12.92 * c
    return 1.055 * (c ** (1 / 2.4)) - 0.055


def srgb_to_oklab(rgb):
    r, g, b = (srgb_to_linear(x) for x in rgb)
    l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b
    m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b
    s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b
    l_, m_, s_ = (x ** (1 / 3) if x > 0 else -((-x) ** (1 / 3)) for x in (l, m, s))
    return (
        0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
        1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
        0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
    )


def oklab_to_srgb(lab):
    L, a, b = lab
    l_ = L + 0.3963377774 * a + 0.2158037573 * b
    m_ = L - 0.1055613458 * a - 0.0638541728 * b
    s_ = L - 0.0894841775 * a - 1.2914855480 * b
    l, m, s = (x**3 for x in (l_, m_, s_))
    r = 4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s
    g = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s
    bb = -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s
    return tuple(min(1.0, max(0.0, linear_to_srgb(x))) for x in (r, g, bb))


def hsl_to_rgb(h, s, l):
    h = h % 360.0
    c = (1 - abs(2 * l - 1)) * s
    x = c * (1 - abs((h / 60.0) % 2 - 1))
    m = l - c / 2
    if h < 60:
        r, g, b = c, x, 0
    elif h < 120:
        r, g, b = x, c, 0
    elif h < 180:
        r, g, b = 0, c, x
    elif h < 240:
        r, g, b = 0, x, c
    elif h < 300:
        r, g, b = x, 0, c
    else:
        r, g, b = c, 0, x
    return (r + m, g + m, b + m)


def num(tok):
    tok = tok.strip()
    m = re.fullmatch(r"calc\(\s*1\s*\*\s*([-\d.]+)(%?)\s*\)", tok)
    if m:
        tok = m.group(1) + m.group(2)
    if tok.endswith("%"):
        return float(tok[:-1]) / 100.0
    if tok.endswith("deg"):
        return float(tok[:-3])
    return float(tok)


NAMED_COLORS = {
    "transparent": (0.0, 0.0, 0.0, 0.0),
    "white": (1.0, 1.0, 1.0, 1.0),
    "black": (0.0, 0.0, 0.0, 1.0),
}


def parse_color(s):
    s = s.strip()
    if s.startswith("#"):
        h = s[1:]
        if len(h) == 3:
            h = "".join(c * 2 for c in h)
        v = [int(h[i : i + 2], 16) / 255.0 for i in range(0, len(h), 2)]
        return tuple(v) if len(v) == 4 else (v[0], v[1], v[2], 1.0)

    m = re.fullmatch(r"(rgba?|hsla?|oklab|color-mix)\((.*)\)", s, flags=re.S)
    if not m:
        if s in NAMED_COLORS:
            return NAMED_COLORS[s]
        raise ValueError("unhandled color: %r" % s)
    fn, args = m.group(1), m.group(2)

    if fn == "color-mix":
        parts = split_top(args)
        space = parts[0].strip()
        if not space.startswith("in "):
            raise ValueError("unhandled color-mix: %r" % s)
        space = space[3:].strip()
        entries = []
        for p in parts[1:]:
            toks = split_space_top(p)
            if len(toks) >= 2 and toks[-1].endswith("%"):
                entries.append((parse_color(" ".join(toks[:-1])), num(toks[-1])))
            else:
                entries.append((parse_color(p), None))
        known = sum(w for _, w in entries if w is not None)
        missing = [i for i, (_, w) in enumerate(entries) if w is None]
        weights = [
            w if w is not None else max(0.0, (1.0 - known) / len(missing))
            for _, w in entries
        ]
        total = sum(weights) or 1.0
        weights = [w / total for w in weights]
        alpha = sum(c[3] * w for (c, _), w in zip(entries, weights))
        if space.startswith("oklab") or space.startswith("oklch"):
            labs = [srgb_to_oklab(c[:3]) for c, _ in entries]
            lab = tuple(sum(l[i] * w for l, w in zip(labs, weights)) for i in range(3))
            rgb = oklab_to_srgb(lab)
        else:
            rgb = tuple(
                sum(c[i] * w for (c, _), w in zip(entries, weights)) for i in range(3)
            )
        return (rgb[0], rgb[1], rgb[2], alpha)

    slash = split_top(args, "/")
    alpha = num(slash[1]) if len(slash) > 1 else None
    core = split_top(slash[0]) if "," in slash[0] else split_space_top(slash[0])
    if fn in ("rgb", "rgba"):
        vals = [
            num(c) if c.strip().endswith("%") else num(c) / 255.0 for c in core[:3]
        ]
        a = alpha if alpha is not None else (num(core[3]) if len(core) > 3 else 1.0)
        return (vals[0], vals[1], vals[2], a)
    if fn in ("hsl", "hsla"):
        a = alpha if alpha is not None else (num(core[3]) if len(core) > 3 else 1.0)
        rgb = hsl_to_rgb(num(core[0]), num(core[1]), num(core[2]))
        return (rgb[0], rgb[1], rgb[2], a)
    if fn == "oklab":
        rgb = oklab_to_srgb(tuple(num(c) for c in core[:3]))
        a = alpha if alpha is not None else 1.0
        return (rgb[0], rgb[1], rgb[2], a)
    raise ValueError("unhandled color function: %r" % s)


DIRECTIONS = {
    "to top": 0.0,
    "to right": 90.0,
    "to bottom": 180.0,
    "to left": 270.0,
    "to bottom right": 135.0,
    "to bottom left": 225.0,
    "to top right": 45.0,
    "to top left": 315.0,
}


def parse_layer(layer):
    layer = layer.strip()
    if not layer.startswith("linear-gradient("):
        return {"kind": "solid", "color": parse_color(layer)}

    depth, end = 0, None
    for i, ch in enumerate(layer):
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
            if depth == 0:
                end = i
                break
    args = layer[len("linear-gradient(") : end]
    rest = layer[end + 1 :].strip()

    parts = split_top(args)
    angle = 180.0
    head = parts[0].strip()
    if re.fullmatch(r"[-\d.]+deg", head):
        angle = num(head)
        parts = parts[1:]
    elif head in DIRECTIONS:
        angle = DIRECTIONS[head]
        parts = parts[1:]

    stops = []
    for p in parts:
        toks = split_space_top(p)
        if len(toks) >= 2 and toks[-1].endswith("%"):
            stops.append([parse_color(" ".join(toks[:-1])), num(toks[-1])])
        else:
            stops.append([parse_color(p), None])
    last = len(stops) - 1
    for i, stop in enumerate(stops):
        if stop[1] is None:
            stop[1] = 0.0 if i == 0 else (1.0 if i == last else i / last)

    return {
        "kind": "gradient",
        "angle": angle,
        "stops": stops,
        "fixed": "fixed" in rest,
    }


def source_over(top, bottom):
    if bottom is None:
        return top
    a = top[3] + bottom[3] * (1.0 - top[3])
    if a <= 1e-6:
        return (0.0, 0.0, 0.0, 0.0)
    return tuple(
        (top[i] * top[3] + bottom[i] * bottom[3] * (1.0 - top[3])) / a for i in range(3)
    ) + (a,)


def is_flat(stops):
    for i in range(3):
        lo = min(s[0][i] for s in stops)
        hi = max(s[0][i] for s in stops)
        if hi - lo > FLAT_EPSILON:
            return False
    return max(s[0][3] for s in stops) - min(s[0][3] for s in stops) <= FLAT_EPSILON


def average(stops):
    n = len(stops)
    return tuple(sum(s[0][i] for s in stops) / n for i in range(4))


def resolve(value):
    """Collapse a CSS background layer list into (angle, stops, overlay, base, fixed)."""
    layers = [parse_layer(l) for l in split_top(value)]
    base = None
    overlay = None
    gradient = None

    for layer in reversed(layers):
        if layer["kind"] == "solid":
            color = layer["color"]
            if gradient is None:
                base = source_over(color, base)
            else:
                overlay = source_over(color, overlay)
            continue

        if is_flat(layer["stops"]):
            color = average(layer["stops"])
            if gradient is None:
                base = source_over(color, base)
            else:
                overlay = source_over(color, overlay)
            continue

        if gradient is not None:
            if all(s[0][3] >= 0.999 for s in layer["stops"]):
                base, overlay = None, None
            else:
                base = source_over(average(gradient["stops"]), base)
        gradient = layer

    if gradient is None:
        return None

    stops = gradient["stops"]
    if all(s[0][3] >= 0.999 for s in stops):
        base = None
    return {
        "angle": gradient["angle"],
        "stops": stops,
        "overlay": overlay,
        "base": base,
        "fixed": gradient["fixed"],
    }


def rgba_literal(color, indent):
    pad = " " * indent
    return (
        "Rgba {\n"
        "%s    r: %s,\n"
        "%s    g: %s,\n"
        "%s    b: %s,\n"
        "%s    a: %s,\n"
        "%s}" % (pad, f(color[0]), pad, f(color[1]), pad, f(color[2]), pad, f(color[3]), pad)
    )


def f(x):
    text = "%.5f" % x
    text = text.rstrip("0")
    if text.endswith("."):
        text += "0"
    if text in ("-0.0", "0.0"):
        return "0.0"
    return text


def emit_gradient(field, spec, indent):
    pad = " " * indent
    lines = ["%s%s: Some(SurfaceGradient {" % (pad, field)]
    lines.append("%s    angle: %s," % (pad, f(spec["angle"])))
    lines.append("%s    stops: &[" % pad)
    for color, position in spec["stops"]:
        lines.append("%s        GradientStop {" % pad)
        lines.append(
            "%s            color: %s," % (pad, rgba_literal(color, indent + 12))
        )
        lines.append("%s            position: %s," % (pad, f(position)))
        lines.append("%s        }," % pad)
    lines.append("%s    ]," % pad)
    for name in ("overlay", "base"):
        color = spec[name]
        if color is None:
            lines.append("%s    %s: None," % (pad, name))
        else:
            lines.append(
                "%s    %s: Some(%s)," % (pad, name, rgba_literal(color, indent + 4))
            )
    lines.append(
        "%s    viewport_anchored: %s," % (pad, "true" if spec["fixed"] else "false")
    )
    lines.append("%s})," % pad)
    return lines


def main():
    themes_dir = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_THEMES_DIR
    if not themes_dir.is_dir():
        raise SystemExit("theme css directory not found: %s" % themes_dir)

    arms = []
    for stem, theme_name in THEMES.items():
        css = themes_dir / ("%s.css" % stem)
        if not css.is_file():
            raise SystemExit("missing theme css: %s" % css)
        decls = declarations(css.read_text())
        entries = []
        emitted = 0
        for prop, field in SURFACES:
            value = decls.get(prop)
            if not value or "linear-gradient" not in value:
                continue
            spec = resolve(value)
            if spec is None:
                continue
            entries.extend(emit_gradient(field, spec, 12))
            emitted += 1
        if not entries:
            continue
        arms.append('        "%s" => SurfaceGradients {' % theme_name)
        arms.extend(entries)
        if emitted < len(SURFACES):
            arms.append("            ..SurfaceGradients::NONE")
        arms.append("        },")

    body = "\n".join(arms)
    OUT.write_text(
        "use gpui::Rgba;\n"
        "\n"
        "use crate::surface::{GradientStop, SurfaceGradient, SurfaceGradients};\n"
        "\n"
        "pub(crate) fn surface_gradients(theme: &str) -> SurfaceGradients {\n"
        "    match theme {\n"
        "%s\n"
        "        _ => SurfaceGradients::NONE,\n"
        "    }\n"
        "}\n" % body
    )
    print("wrote %s" % OUT)


if __name__ == "__main__":
    main()
