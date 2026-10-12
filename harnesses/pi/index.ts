/**
 * annox for pi: runs `annox watch` and hands each thread it prints to the
 * agent as a message, so the agent answers threads addressed to it while you
 * write them. The pi counterpart of `annox mcp --channel` in Claude Code.
 *
 *   pi --annox-watch=@pi      start watching with the session (with "=": pi
 *                             reads a separate @pi as a file to attach)
 *   /annox-watch [TEXT]       start watching in a running session (default @pi)
 *   /annox-watch off          stop
 */

import { type ChildProcess, spawn } from "node:child_process";
import { createInterface } from "node:readline";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { Box, Text } from "@earendil-works/pi-tui";

const DEFAULT_MENTION = "@pi";

const INSTRUCTIONS = `A thread addressed to you in annox changed. It's from someone reviewing the file: act on it as they ask. \
If the annox skill is available and you haven't loaded it, load it first. \
Answer with \`annox reply\`; propose changes to their prose with \`annox suggest\`, not by editing the file; \
then mark the thread read with \`annox read ID\`. If the thread needs nothing from you, just mark it read. \
The thread, as \`annox list\` prints it:`;

interface Thread {
	id: string;
	path: string;
	line?: number;
	kind: string;
	author: { id: string; name?: string | null };
	body?: string | null;
	replies?: { author: { id: string; name?: string | null }; body?: string | null }[];
}

export default function (pi: ExtensionAPI) {
	let watch: ChildProcess | undefined;

	pi.registerFlag("annox-watch", {
		description: "Watch for annox threads that mention TEXT, such as @pi, and answer them",
		type: "string",
	});

	pi.registerMessageRenderer<Thread>("annox", (message, { expanded, outputPad }, theme) => {
		const thread = message.details;
		let text = theme.fg("accent", "annox");
		if (thread) {
			const latest = thread.replies?.at(-1) ?? thread;
			const who = latest.author.name ?? latest.author.id;
			const where = thread.line ? `${thread.path}:${thread.line}` : thread.path;
			const body = (latest.body ?? "").split("\n")[0];
			text += ` ${theme.fg("dim", `${thread.kind} on ${where}`)}\n${who}: ${body}`;
		}
		if (expanded && typeof message.content === "string") text += `\n\n${theme.fg("dim", message.content)}`;
		const box = new Box(outputPad, 1, (t) => theme.bg("customMessageBg", t));
		box.addChild(new Text(text, 0, 0));
		return box;
	});

	function stop(ctx?: ExtensionContext) {
		watch?.kill();
		watch = undefined;
		ctx?.ui.setStatus("annox", undefined);
	}

	function start(mention: string, ctx: ExtensionContext) {
		stop(ctx);
		// The agent runs annox through the shell, which inherits this
		// environment: it writes as the identity the watch follows, so its own
		// replies never wake it again.
		process.env.ANNOX_AUTHOR ??= "urn:agent:pi";
		if (!process.env.ANNOX_AUTHOR_NAME && process.env.ANNOX_AUTHOR === "urn:agent:pi") {
			process.env.ANNOX_AUTHOR_NAME = "Pi";
		}

		const child = spawn("annox", ["watch", "--mention", mention], {
			cwd: ctx.cwd,
			stdio: ["ignore", "pipe", "pipe"],
		});
		watch = child;
		ctx.ui.setStatus("annox", `annox: watching ${mention}`);

		createInterface({ input: child.stdout! }).on("line", (line) => {
			let thread: Thread;
			try {
				thread = JSON.parse(line);
			} catch {
				return;
			}
			pi.sendMessage(
				{
					customType: "annox",
					content: `${INSTRUCTIONS}\n\n${JSON.stringify(thread, null, 2)}`,
					display: true,
					details: thread,
				},
				// Starts a turn when idle; waits for the current one otherwise.
				{ triggerTurn: true, deliverAs: "followUp" },
			);
		});

		let stderr = "";
		child.stderr!.on("data", (chunk) => {
			stderr += chunk;
		});
		child.on("error", (e) => {
			if (watch !== child) return;
			watch = undefined;
			ctx.ui.setStatus("annox", undefined);
			ctx.ui.notify(`annox watch couldn't start: ${e.message}`, "error");
		});
		child.on("exit", (code) => {
			if (watch !== child) return;
			watch = undefined;
			ctx.ui.setStatus("annox", undefined);
			ctx.ui.notify(`annox watch stopped (exit ${code}): ${stderr.trim()}`, "error");
		});
	}

	pi.registerCommand("annox-watch", {
		description: `Answer annox threads that mention TEXT (default ${DEFAULT_MENTION}); "off" stops`,
		handler: async (args, ctx) => {
			const arg = args.trim();
			if (arg === "off") {
				stop(ctx);
				ctx.ui.notify("annox watch stopped", "info");
				return;
			}
			start(arg || DEFAULT_MENTION, ctx);
		},
	});

	pi.on("session_start", async (_event, ctx) => {
		const mention = pi.getFlag("annox-watch");
		if (typeof mention === "string" && mention) start(mention, ctx);
	});

	pi.on("session_shutdown", async (_event, ctx) => stop(ctx));
}
