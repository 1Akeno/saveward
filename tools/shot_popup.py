import sys
from playwright.sync_api import sync_playwright

out = sys.argv[1]
states = {
    "backup": "stage=backup&done=28000000&total=46000000",
    "upload": "stage=upload&done=12000000&total=44000000",
    "done": "stage=done",
    "failed": "stage=upload-failed&msg=network%20problem:%20connection%20refused",
}
with sync_playwright() as p:
    b = p.chromium.launch()
    for scheme in ("light", "dark"):
        for name, q in states.items():
            page = b.new_page(viewport={"width": 360, "height": 108}, color_scheme=scheme)
            errors = []
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.goto(f"http://localhost:1420/popup.html?{q}")
            page.wait_for_timeout(1500)
            page.screenshot(path=f"{out}-{scheme}-{name}.png")
            if errors:
                print(scheme, name, "errors:", errors)
    b.close()
print("ok")
