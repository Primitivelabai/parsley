"""Regenerate src/report-fonts.css: Latin subsets of Geist and Geist Mono embedded
as data URIs, so saved reports look the same offline. Needs fonttools and brotli."""
import base64
from pathlib import Path
from fontTools import subset
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parent.parent
UNICODES = "U+0020-007E,U+00A0-00FF,U+2013-2014,U+2018-201D,U+2022,U+2026,U+2190,U+2192,U+00B7,U+2212,U+00D7,U+2264,U+2265"

def embed(source):
    options = subset.Options()
    options.flavor = "woff2"
    options.layout_features = ["kern", "liga", "tnum"]
    options.name_IDs = ["*"]
    font = TTFont(ROOT / "ui" / source)
    subsetter = subset.Subsetter(options)
    subsetter.populate(unicodes=subset.parse_unicodes(UNICODES))
    subsetter.subset(font)
    font.flavor = "woff2"
    out = ROOT / "target" / (source + ".subset")
    out.parent.mkdir(exist_ok=True)
    font.save(out)
    return base64.b64encode(out.read_bytes()).decode()

(ROOT / "src" / "report-fonts.css").write_text(
    "/* Geist and Geist Mono, subset to Latin and embedded so reports look the same offline.\n"
    "   SIL Open Font License 1.1, see ui/OFL-LICENSE.txt. Regenerate with tools/report-fonts.py. */\n"
    f"@font-face{{font-family:ParsleyGeist;src:url(data:font/woff2;base64,{embed('geist.woff2')}) format('woff2');font-weight:100 900;font-display:swap}}\n"
    f"@font-face{{font-family:ParsleyMono;src:url(data:font/woff2;base64,{embed('mono.woff2')}) format('woff2');font-weight:100 900;font-display:swap}}\n"
)
