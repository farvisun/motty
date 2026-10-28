// Motty: tells Motty's tab what this agent is doing (working, waiting for
// you, done). Added by Motty's Settings → Agents; turning the switch off
// there deletes this file.
import { spawn } from "node:child_process";

const CLI = {{CLI}};

let queue: Promise<unknown> = Promise.resolve();
let last: string | undefined;

function report(state: string, extra: string[] = []) {
  if (!process.env.MOTTY_PANE_ID || (state === last && extra.length === 0)) return;
  last = state;
  const args = ["hook", "{{ID}}", state, ...extra];
  queue = queue.then(
    () =>
      new Promise<void>((done) => {
        try {
          const child = spawn(CLI, args, { stdio: "ignore" });
          child.on("error", () => done());
          child.on("exit", () => done());
        } catch {
          done();
        }
      }),
  );
}

function session(ctx: any): string[] {
  try {
    const id = ctx?.sessionManager?.getSessionId?.();
    return id ? [`session=${id}`] : [];
  } catch {
    return [];
  }
}

export default function (pi: any) {
  // Only the interactive agent owns a tab; print and RPC modes don't.
  let tui = false;

  pi.on("session_start", async (_event: any, ctx: any) => {
    tui = ctx?.mode === undefined || ctx?.mode === "tui";
    if (tui) report("idle", session(ctx));
  });

  pi.on("before_agent_start", async (event: any, ctx: any) => {
    if (!tui) return;
    const prompt = typeof event?.prompt === "string" ? event.prompt.split("\n")[0].slice(0, 160) : "";
    report("processing", [...session(ctx), ...(prompt ? [`message=${prompt}`] : [])]);
  });

  pi.on("ui_prompt_start", async () => {
    if (tui) report("awaiting");
  });

  pi.on("ui_prompt_end", async () => {
    if (tui) report("processing");
  });

  pi.on("{{SETTLED}}", async (_event: any, ctx: any) => {
    if (tui && ctx?.isIdle?.() !== false) report("idle");
  });

  pi.on("session_shutdown", async () => {
    if (tui) report("ended");
  });
}
