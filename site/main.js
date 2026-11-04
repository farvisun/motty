// Motty landing page: terminal typing, theme tabs, copy buttons.
// All content is in the HTML; this script only enhances it.
(() => {
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const $ = (s, el = document) => el.querySelector(s);
  const $$ = (s, el = document) => [...el.querySelectorAll(s)];
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

  // Terminal: type the commands once when the snippet scrolls into view.
  // The full transcript is already in the HTML, so without JS or with
  // reduced motion it simply stays as is.
  const code = $("[data-typer] code");
  if (code && !reduced && "IntersectionObserver" in window) {
    const pre = code.parentElement;
    const lines = code.innerHTML.split("\n");
    const caret = '<span class="caret" aria-hidden="true"></span>';

    const play = async () => {
      pre.style.minHeight = `${pre.offsetHeight}px`; // no layout shift while typing
      let html = "";
      for (const line of lines) {
        const tmp = document.createElement("div");
        tmp.innerHTML = line;
        const cmd = $(".c", tmp);
        if (!cmd) {
          html += line + "\n";
          code.innerHTML = html + caret;
          await sleep(line.trim() ? 140 : 80);
          continue;
        }
        const text = cmd.textContent;
        const head = line.slice(0, line.indexOf('<span class="c">'));
        for (let i = 1; i <= text.length; i++) {
          cmd.textContent = text.slice(0, i);
          code.innerHTML = html + head + cmd.outerHTML + caret;
          await sleep(14 + Math.random() * 22);
        }
        html += line + "\n";
        await sleep(260);
      }
      code.innerHTML = html.replace(/\n$/, "") + caret;
    };

    const io = new IntersectionObserver((entries) => {
      if (entries.some((e) => e.isIntersecting)) { io.disconnect(); play(); }
    }, { threshold: 0.4 });
    io.observe(pre);
  }

  // Theme tabs: swap the screenshot. Arrow keys, Home and End move between tabs.
  const tabs = $$('.segmented [role="tab"]');
  const panel = $("#theme-panel");
  const shot = panel && $("img", panel);
  const src = (t) => `assets/shots/theme-${t}.webp`;

  const select = (tab, focus = false) => {
    tabs.forEach((t) => {
      const on = t === tab;
      t.setAttribute("aria-selected", String(on));
      t.tabIndex = on ? 0 : -1;
    });
    if (focus) tab.focus();
    panel.setAttribute("aria-labelledby", tab.id);
    const next = src(tab.dataset.theme);
    if (shot.getAttribute("src") === next) return;
    panel.classList.add("loading");
    const done = () => panel.classList.remove("loading");
    shot.addEventListener("load", done, { once: true });
    shot.addEventListener("error", done, { once: true });
    shot.src = next;
    shot.alt = `Motty in the ${tab.textContent.trim()} theme.`;
  };

  tabs.forEach((tab, i) => {
    tab.addEventListener("click", () => select(tab));
    tab.addEventListener("keydown", (e) => {
      const keys = { ArrowRight: i + 1, ArrowDown: i + 1, ArrowLeft: i - 1, ArrowUp: i - 1, Home: 0, End: tabs.length - 1 };
      if (!(e.key in keys)) return;
      e.preventDefault();
      select(tabs[(keys[e.key] + tabs.length) % tabs.length], true);
    });
    // Warm the cache on first hover so the swap is instant.
    tab.addEventListener("pointerenter", () => { new Image().src = src(tab.dataset.theme); }, { once: true });
  });

  // Copy buttons.
  $$("[data-copy]").forEach((btn) => {
    const label = btn.textContent;
    btn.addEventListener("click", async () => {
      const text = $(btn.dataset.copy).innerText.trim();
      try {
        await navigator.clipboard.writeText(text);
        btn.textContent = "Copied";
      } catch {
        const range = document.createRange();
        range.selectNodeContents($(btn.dataset.copy));
        getSelection().removeAllRanges();
        getSelection().addRange(range);
        btn.textContent = "Press ⌘C";
      }
      setTimeout(() => { btn.textContent = label; }, 1600);
    });
  });
})();
