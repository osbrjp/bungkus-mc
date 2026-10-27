import { spawn } from "node:child_process";

const HOOK_TIMEOUT_MS = 2000;

export default function bungkusMc(pi) {
  let totals = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: 0 };

  const common = (ctx) => ({
    session_id: ctx.sessionManager.getSessionId(),
    session_title: pi.getSessionName(),
    cwd: ctx.cwd,
  });

  const send = (payload) => {
    const executable = process.env.BUNGKUS_MC_EXE;
    if (!executable || !process.env.BUNGKUS_MC_SOCK || !process.env.BUNGKUS_MC_SESSION) {
      return Promise.resolve();
    }
    return new Promise((resolve) => {
      let finished = false;
      let timer;
      const done = () => {
        if (!finished) {
          finished = true;
          clearTimeout(timer);
          resolve();
        }
      };
      const child = spawn(executable, ["pi-hook"], {
        env: process.env,
        shell: false,
        stdio: ["pipe", "ignore", "ignore"],
      });
      timer = setTimeout(() => {
        child.kill("SIGTERM");
        done();
      }, HOOK_TIMEOUT_MS);
      timer.unref();
      child.once("error", done);
      child.once("close", done);
      child.stdin.on("error", () => {});
      child.stdin.end(JSON.stringify(payload));
    });
  };

  pi.on("session_start", (_event, ctx) => {
    totals = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: 0 };
    return send({ hook_event_name: "SessionStart", ...common(ctx) });
  });

  pi.on("session_info_changed", (event, ctx) =>
    send({ hook_event_name: "SessionInfo", ...common(ctx), session_title: event.name }),
  );

  pi.on("before_agent_start", (_event, ctx) =>
    send({ hook_event_name: "UserPromptSubmit", ...common(ctx) }),
  );

  pi.on("tool_execution_start", (event, ctx) =>
    send({
      hook_event_name: "PreToolUse",
      ...common(ctx),
      tool_name: event.toolName,
      tool_use_id: event.toolCallId,
    }),
  );

  pi.on("tool_execution_end", (event, ctx) =>
    send({
      hook_event_name: "PostToolUse",
      ...common(ctx),
      tool_name: event.toolName,
      tool_use_id: event.toolCallId,
    }),
  );

  pi.on("ui_prompt_start", (_event, ctx) =>
    send({
      hook_event_name: "Notification",
      ...common(ctx),
      notification_type: "agent_needs_input",
    }),
  );

  pi.on("ui_prompt_end", (_event, ctx) =>
    send({ hook_event_name: "UserPromptSubmit", ...common(ctx) }),
  );

  pi.on("message_end", (event, ctx) => {
    if (event.message.role !== "assistant") return;
    const usage = event.message.usage;
    totals.input += usage.input;
    totals.output += usage.output;
    totals.cacheRead += usage.cacheRead;
    totals.cacheWrite += usage.cacheWrite;
    totals.cost += usage.cost.total;
    const context = ctx.getContextUsage();
    return send({
      bungkus_mc_pi_usage: {
        ...totals,
        contextPercent: context?.percent,
        contextWindow: context?.contextWindow,
        sessionName: pi.getSessionName(),
      },
    });
  });

  pi.on("agent_settled", (_event, ctx) =>
    send({ hook_event_name: "Stop", ...common(ctx) }),
  );

  pi.on("session_shutdown", (_event, ctx) =>
    send({ hook_event_name: "SessionEnd", ...common(ctx) }),
  );
}
