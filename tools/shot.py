import sys
from playwright.sync_api import sync_playwright

out = sys.argv[1] if len(sys.argv) > 1 else "shots"
select = sys.argv[2] if len(sys.argv) > 2 and sys.argv[2] else None
click = sys.argv[3] if len(sys.argv) > 3 and sys.argv[3] else None
query = sys.argv[4] if len(sys.argv) > 4 else ""
url = "http://localhost:1420/" + query

with sync_playwright() as p:
    b = p.chromium.launch()
    for scheme in ("light", "dark"):
        page = b.new_page(viewport={"width": 1100, "height": 700}, color_scheme=scheme)
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.on("console", lambda m: m.type == "error" and errors.append(m.text))
        page.goto(url)
        page.wait_for_selector("h1, .empty", timeout=10000)
        if select:
            page.click(f'.g[data-name="{select}"]')
            page.wait_for_timeout(200)
        if click:
            page.click(click)
        page.wait_for_timeout(2500)
        page.screenshot(path=f"{out}-{scheme}.png")
        print(scheme, "errors:", errors)
    b.close()
