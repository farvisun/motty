// Motty landing page: reveals, the robot's eyes, the typed terminal, theme picker.
(() => {
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;
  const $ = (s, el = document) => el.querySelector(s);
  const $$ = (s, el = document) => [...el.querySelectorAll(s)];

  // Nav background once the page scrolls.
  const nav = $(".nav");
  const onScroll = () => nav.classList.toggle("scrolled", scrollY > 24);
  addEventListener("scroll", onScroll, { passive: true });
  onScroll();

  // Reveal on scroll, staggered among siblings that enter together.
  const io = new IntersectionObserver((entries) => {
    entries.filter((e) => e.isIntersecting).forEach((e, i) => {
      e.target.style.setProperty("--d", `${Math.min(i * 0.08, 0.4)}s`);
      e.target.classList.add("in");
      io.unobserve(e.target);
    });
  }, { rootMargin: "0px 0px -8% 0px", threshold: 0.12 });
  $$(".reveal").forEach((el) => io.observe(el));

  // Hero screenshot lies back and straightens as it scrolls into view.
  const frame = $(".stage-frame");
  if (frame && !reduced) {
    let ticking = false;
    const tilt = () => {
      const r = frame.getBoundingClientRect();
      const t = Math.min(Math.max((r.top - innerHeight * 0.18) / (innerHeight * 0.6), 0), 1);
      frame.style.setProperty("--tilt", t.toFixed(3));
      ticking = false;
    };
    addEventListener("scroll", () => { if (!ticking) { ticking = true; requestAnimationFrame(tilt); } }, { passive: true });
    addEventListener("resize", tilt);
    tilt();
  } else if (frame) {
    frame.style.setProperty("--tilt", 0);
  }

  // The robot looks at the pointer, blinks, and smiles at the buttons.
  const bot = $(".bot");
  if (bot) {
    const shell = $(".bot-shell", bot);
    const eyes = $$(".eye, .mouth", bot);
    const look = (x, y) => {
      const r = shell.getBoundingClientRect();
      const dx = x - (r.left + r.width / 2);
      const dy = y - (r.top + r.height / 2);
      const d = Math.hypot(dx, dy) || 1;
      const reach = r.width * 0.07;
      const k = Math.min(d / 400, 1);
      const ex = (dx / d) * reach * k;
      const ey = (dy / d) * reach * 0.7 * k;
      eyes.forEach((e) => { e.style.setProperty("--ex", `${ex}px`); e.style.setProperty("--ey", `${ey}px`); });
      shell.style.setProperty("--ry", `${(dx / d) * 10 * k}deg`);
      shell.style.setProperty("--rx", `${-(dy / d) * 8 * k}deg`);
    };
    if (!reduced) {
      addEventListener("pointermove", (e) => look(e.clientX, e.clientY), { passive: true });
      const blink = () => {
        $$(".eye", bot).forEach((e) => e.style.setProperty("--blink", 0.08));
        setTimeout(() => $$(".eye", bot).forEach((e) => e.style.setProperty("--blink", 1)), 130);
        setTimeout(blink, 2400 + Math.random() * 3600);
      };
      setTimeout(blink, 1800);
    }
    $$("[data-smile]").forEach((b) => {
      b.addEventListener("pointerenter", () => bot.classList.add("smile"));
      b.addEventListener("pointerleave", () => bot.classList.remove("smile"));
      b.addEventListener("focus", () => bot.classList.add("smile"));
      b.addEventListener("blur", () => bot.classList.remove("smile"));
    });
  }

  // Typed terminal: commands type out, output appears, then it loops.
  const typer = $("[data-typer] code");
  if (typer) {
    const script = [
      ["cmd", "claude mcp add motty -- motty mcp"],
      ["out", "Added stdio MCP server motty with command: motty mcp", "ok"],
      ["gap"],
      ["cmd", "motty pane split -d down --command \"cargo test\""],
      ["out", "7", "hl"],
      ["cmd", "motty pane wait --pane 7"],
      ["cmd", "motty pane capture --pane 7 --last-command | tail -3"],
      ["out", "test theme::tests::missing_palette_is_error ... ok", "o"],
      ["out", "test result: ok. 169 passed; 0 failed; 1 ignored", "ok"],
      ["out", "     finished in 1.49s", "o"],
      ["gap"],
      ["cmd", "motty watch -- cargo build --release"],
      ["out", "⠋ building on tab t_2 … you'll get a badge and a notification", "o"],
    ];
    const esc = (s) => s.replace(/[&<>]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;" }[c]));
    const prompt = '<span class="p">❯</span> ';
    let html = "";
    const render = (tail = "") => { typer.innerHTML = html + tail + '<span class="caret"></span>'; };
    const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

    const run = async () => {
      for (;;) {
        html = "";
        for (const [kind, text, cls] of script) {
          if (kind === "gap") { html += "\n"; render(); await sleep(250); continue; }
          if (kind === "out") { html += `<span class="${cls}">${esc(text)}</span>\n`; render(); await sleep(220); continue; }
          let typed = "";
          for (const ch of text) {
            typed += ch;
            render(prompt + `<span class="c">${esc(typed)}</span>`);
            await sleep(18 + Math.random() * 45);
          }
          await sleep(380);
          html += prompt + `<span class="c">${esc(text)}</span>\n`;
          render();
          await sleep(320);
        }
        html += prompt;
        render();
        await sleep(5200);
      }
    };

    if (reduced) {
      html = script.map(([k, t, c]) => k === "gap" ? "" : k === "out" ? `<span class="${c}">${esc(t)}</span>` : prompt + `<span class="c">${esc(t)}</span>`).join("\n") + "\n" + prompt;
      render();
    } else {
      const start = new IntersectionObserver((es) => {
        if (es.some((e) => e.isIntersecting)) { start.disconnect(); run(); }
      }, { threshold: 0.3 });
      start.observe(typer);
      render();
    }
  }

  // Theme picker: swap the screenshot with a short crossfade.
  const view = $(".theme-view");
  const picker = $$(".theme-picker button");
  picker.forEach((btn) => {
    const img = new Image();
    btn.addEventListener("pointerenter", () => { img.src = `assets/shots/theme-${btn.dataset.theme}.webp`; }, { once: true });
    btn.addEventListener("click", () => {
      picker.forEach((b) => b.setAttribute("aria-selected", String(b === btn)));
      const target = $("img", view);
      view.classList.add("swap");
      setTimeout(() => {
        target.src = `assets/shots/theme-${btn.dataset.theme}.webp`;
        target.alt = `Motty in the ${btn.textContent.trim()} theme.`;
        const done = () => view.classList.remove("swap");
        if (target.complete) requestAnimationFrame(done); else target.onload = done;
      }, reduced ? 0 : 220);
    });
  });
  // Arrow keys move between theme tabs.
  $(".theme-picker")?.addEventListener("keydown", (e) => {
    if (!["ArrowLeft", "ArrowRight"].includes(e.key)) return;
    const i = picker.indexOf(document.activeElement);
    if (i < 0) return;
    const next = picker[(i + (e.key === "ArrowRight" ? 1 : picker.length - 1)) % picker.length];
    next.focus();
    next.click();
  });

  // Copy buttons.
  $$("[data-copy]").forEach((btn) => {
    btn.addEventListener("click", async () => {
      const text = $(btn.dataset.copy).innerText.trim();
      try {
        await navigator.clipboard.writeText(text);
        btn.textContent = "Copied";
      } catch {
        btn.textContent = "Select & copy";
      }
      setTimeout(() => { btn.textContent = "Copy"; }, 1600);
    });
  });

  // Spotlight that follows the pointer across bento cards.
  $$(".card").forEach((card) => {
    card.addEventListener("pointermove", (e) => {
      const r = card.getBoundingClientRect();
      card.style.setProperty("--mx", `${e.clientX - r.left}px`);
      card.style.setProperty("--my", `${e.clientY - r.top}px`);
    });
  });
})();
