// Motty: tells Motty's tab what OpenCode is doing (working, waiting for
// approval, done). Added by Motty's Settings → Agents; turning the switch
// off there deletes this file.
import { spawn } from "node:child_process";

const CLI = {{CLI}};

// Reports go out one at a time, in order: a late "busy" must not land
// after the "idle" that ended the turn.
let queue = Promise.resolve();
let last;

function report(state, session) {
  if (!process.env.MOTTY_PANE_ID || state === last) return queue;
  last = state;
  const args = ["hook", "opencode", state];
  if (session) args.push(`session=${session}`);
  queue = queue.then(
    () =>
      new Promise((done) => {
        try {
          const child = spawn(CLI, args, { stdio: "ignore" });
          child.on("error", done);
          child.on("exit", done);
        } catch {
          done();
        }
      }),
  );
  return queue;
}

// Subagents' sessions report through their parent.
const parents = new Map();

function root(id) {
  while (id && parents.has(id)) id = parents.get(id);
  return id;
}

export const MottyPlugin = async () => ({
  event: async ({ event }) => {
    const props = event?.properties ?? {};
    const info = props.info;
    if (info?.id && info.parentID) parents.set(info.id, info.parentID);
    const id = props.sessionID ?? info?.id;
    const session = root(id);
    const child = id !== session;
    switch (event?.type) {
      case "session.status": {
        const type = props.status?.type;
        if (type === "busy" || type === "retry") await report("processing", session);
        break;
      }
      case "permission.asked":
      case "permission.updated":
      case "question.asked":
        await report("awaiting", session);
        break;
      case "permission.replied":
      case "question.replied":
      case "question.rejected":
        await report("processing", session);
        break;
      case "session.error":
        if (!child) await report("error", session);
        break;
      case "session.idle":
        if (!child) await report("idle", session);
        break;
    }
  },
});

// OpenCode 2 loads a default export with an id; 1 uses the function.
export default { id: "motty", server: MottyPlugin, setup() {} };
