# 由 kwscope.svg 生成三份图标，产物入库，构建时不需要这些 Python 依赖：
#   kwscope.png   1024 见方，运行时的窗口图标（src/main.rs include_bytes!）
#   kwscope.icns  Kwscope.app 的图标（scripts/package.sh）
#   kwscope.ico   Windows exe 的图标（build.rs）
#
#   uv run --with pillow --with cairosvg python packaging/icon/render.py
import io
from pathlib import Path

import cairosvg
from PIL import Image, ImageDraw, ImageFilter

here = Path(__file__).parent
size = 1024

art = Image.open(
    io.BytesIO(cairosvg.svg2png(url=str(here / "kwscope.svg"), output_width=size, output_height=size))
).convert("RGBA")

# macOS 图标模板的投影：黑 30%，向下 10 px，模糊 10 px；白底在浅色背景上靠它显出边
mask = Image.new("L", (size, size), 0)
ImageDraw.Draw(mask).rounded_rectangle((100, 110, 923, 933), radius=185, fill=int(255 * 0.3))
shadow = Image.new("RGBA", (size, size), (0, 0, 0, 0))
shadow.putalpha(mask.filter(ImageFilter.GaussianBlur(10)))

icon = Image.alpha_composite(shadow, art)
icon.save(here / "kwscope.png", optimize=True)
icon.save(here / "kwscope.icns")
icon.save(here / "kwscope.ico", sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
