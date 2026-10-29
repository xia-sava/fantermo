"""Edge で描いたアイコンの PNG から、fantermo.ico と見比べるための見本を作る。tools/icon.sh から呼ぶ。

使い方: python3.12 tools/icon/icon.py <描いた PNG のディレクトリ> <ICO の出力先> [<見本の出力先>]

描いた PNG のディレクトリには、icon.sh が fantermo.svg と fantermo-small.svg を 256 px で描いた
large.png と small.png を置く。各サイズはそれを縮めて作る。

ICO の 256 px は PNG で、ほかは 32 ビット（アルファつき）のビットマップで持つ。Pillow の ICO の書き出しは
すべてのサイズを同じ形式にしかできないので、ここで書く。
"""

import struct
import sys
from io import BytesIO
from pathlib import Path

from PIL import Image

# 描いた PNG の大きさ。SVG の width と height。
SOURCE_SIZE = 256

# ICO に入れるサイズと、どちらの SVG から作るか。24 px 以下は線を太くした small を使う。
SIZES = [
    (16, "small"),
    (20, "small"),
    (24, "small"),
    (32, "large"),
    (40, "large"),
    (48, "large"),
    (64, "large"),
    (96, "large"),
    (256, "large"),
]

# この大きさ以上のサイズを PNG で持つ。
PNG_FROM = 256

# 見本の背景の色（明るい背景、暗い背景）と、サイズの間の空き。
SHEET_BACKGROUNDS = [(243, 243, 243), (32, 32, 32)]
SHEET_GAP = 16
# 小さいサイズが見えるよう、見本を縦横この倍率で拡大する（画素は補間しない）。
SHEET_ZOOM = 2


def load_sources(directory: Path) -> dict[str, Image.Image]:
    sources = {}
    for name in ("large", "small"):
        image = Image.open(directory / f"{name}.png").convert("RGBA")
        if image.size != (SOURCE_SIZE, SOURCE_SIZE):
            sys.exit(f"{name}.png is {image.size}, expected {SOURCE_SIZE}x{SOURCE_SIZE}")
        sources[name] = image
    return sources


def render(sources: dict[str, Image.Image]) -> list[Image.Image]:
    frames = []
    for size, name in SIZES:
        source = sources[name]
        frames.append(source if size == SOURCE_SIZE else source.resize((size, size), Image.LANCZOS))
    return frames


def bitmap_bytes(image: Image.Image) -> bytes:
    """ICO の中のビットマップ（BITMAPINFOHEADER、下の行からの BGRA、AND マスク）。"""
    width, height = image.size
    # 高さは色の部分と AND マスクを合わせた 2 倍で書く。
    header = struct.pack("<IiiHHIIiiII", 40, width, height * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    pixels = image.load()
    color = bytearray()
    for y in reversed(range(height)):
        for x in range(width):
            r, g, b, a = pixels[x, y]
            color += bytes((b, g, r, a))
    # アルファを持つので AND マスクは使われないが、形式として要る。行は 4 バイトの境界にそろえる。
    mask_row = ((width + 31) // 32) * 4
    mask = bytes(mask_row * height)
    return header + bytes(color) + mask


def png_bytes(image: Image.Image) -> bytes:
    buffer = BytesIO()
    image.save(buffer, "png")
    return buffer.getvalue()


def write_ico(frames: list[Image.Image], path: Path) -> None:
    images = [png_bytes(f) if f.width >= PNG_FROM else bitmap_bytes(f) for f in frames]
    out = bytearray(struct.pack("<HHH", 0, 1, len(frames)))
    offset = 6 + 16 * len(frames)
    for frame, data in zip(frames, images):
        # 幅と高さの 0 は 256 を表す。
        side = frame.width if frame.width < 256 else 0
        out += struct.pack("<BBBBHHII", side, side, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    for data in images:
        out += data
    path.write_bytes(bytes(out))


def write_sheet(frames: list[Image.Image], path: Path) -> None:
    """各サイズを明るい背景と暗い背景に並べた見本。"""
    width = sum(f.width for f in frames) + SHEET_GAP * (len(frames) + 1)
    height = SOURCE_SIZE + SHEET_GAP * 2
    sheet = Image.new("RGB", (width, height * len(SHEET_BACKGROUNDS)))
    for row, background in enumerate(SHEET_BACKGROUNDS):
        tile = Image.new("RGB", (width, height), background)
        x = SHEET_GAP
        for frame in frames:
            tile.paste(frame, (x, (height - frame.height) // 2), frame)
            x += frame.width + SHEET_GAP
        sheet.paste(tile, (0, row * height))
    sheet = sheet.resize((sheet.width * SHEET_ZOOM, sheet.height * SHEET_ZOOM), Image.NEAREST)
    sheet.save(path)


def main() -> None:
    if len(sys.argv) not in (3, 4):
        sys.exit(__doc__)
    frames = render(load_sources(Path(sys.argv[1])))
    write_ico(frames, Path(sys.argv[2]))
    if len(sys.argv) == 4:
        write_sheet(frames, Path(sys.argv[3]))


if __name__ == "__main__":
    main()
