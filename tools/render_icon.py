import pathlib, sys
from playwright.sync_api import sync_playwright

root = pathlib.Path(__file__).resolve().parent.parent
svg = (root / "src-tauri" / "icons" / "source.svg").read_text()
out = root / "src-tauri" / "icons" / "source.png"
preview = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else None

with sync_playwright() as p:
    b = p.chromium.launch()
    page = b.new_page(viewport={"width": 1024, "height": 1024})
    page.set_content(f'<style>html,body{{margin:0;background:transparent}}</style>{svg.replace("<svg ", "<svg width=1024 height=1024 ", 1)}')
    page.screenshot(path=str(out), omit_background=True)
    if preview:
        sizes = [256, 64, 32, 16]
        imgs = "".join(f'<div style="display:inline-block;margin:12px;vertical-align:bottom">{svg.replace("<svg ", f"<svg width={s} height={s} ", 1)}</div>' for s in sizes)
        page.set_viewport_size({"width": 520, "height": 300})
        page.set_content(f'<body style="margin:0;background:#e6e8eb"><div>{imgs}</div><div style="background:#1c1d20">{imgs}</div></body>')
        page.screenshot(path=str(preview), full_page=True)
    b.close()
print("wrote", out)
