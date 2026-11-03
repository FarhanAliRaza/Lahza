"""Contact sheet from a rendered video at given times (ffmpeg frame grabs)."""
import subprocess
import sys

from PIL import Image, ImageDraw, ImageFont

video, out = sys.argv[1], sys.argv[2]
times = [float(t) for t in sys.argv[3].split(",")]
cols = int(sys.argv[4]) if len(sys.argv) > 4 else 4
w = 480
h = w * 9 // 16
font = ImageFont.truetype("/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Medium.ttf", 14)
rows = (len(times) + cols - 1) // cols
sheet = Image.new("RGB", (cols * (w + 8) + 8, rows * (h + 30) + 8), "#18181a")
for i, t in enumerate(times):
    png = subprocess.run(["ffmpeg", "-v", "error", "-ss", f"{t:.4f}", "-i", video, "-frames:v", "1", "-vf", f"scale={w}:{h}", "-f", "image2pipe", "-vcodec", "png", "-"],
                         capture_output=True).stdout
    import io
    im = Image.open(io.BytesIO(png)).convert("RGB")
    x, y = 8 + (i % cols) * (w + 8), 8 + (i // cols) * (h + 30)
    sheet.paste(im, (x, y))
    ImageDraw.Draw(sheet).text((x, y + h + 6), f"{t:.3f}s  f{round(t * 30)}", fill="#e8e8e8", font=font)
sheet.save(out)
print(out)
