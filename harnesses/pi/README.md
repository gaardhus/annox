# annox for pi

An extension for [pi](https://pi.dev) that lets the agent answer annox threads addressed to it while you write them. It runs `annox watch` and hands each thread it prints to the agent as a message, which starts a turn when pi is idle and waits for the current one otherwise. It's the pi counterpart of `annox mcp --channel` in Claude Code (see [Live](../../README.md#live-let-the-agent-pick-up-threads-as-you-write-them)).

Requires the `annox` binary on your `PATH` (see the [install script](../../README.md#reference-implementation)). The agent answers through the annox CLI, so install the [skill](../skills/annox/SKILL.md) too: pi reads `~/.agents/skills`, where `install.sh --skill` puts it.

## Setup

From a checkout:

```sh
pi install ./harnesses/pi
```

Or try it for one session with `pi -e ./harnesses/pi`.

## Use

Start pi with the watch on, or turn it on and off in a session:

```sh
pi --annox-watch=@pi
```

```text
/annox-watch            watch for threads that mention @pi
/annox-watch @reviewer  or another text
/annox-watch off        stop
```

Write the flag with `=`: pi reads a separate `@pi` as a file to attach. The footer shows `annox: watching @pi` while it runs.

Then comment `@pi …` in your editor and save. Threads already unread are picked up when the watch starts, and threads the agent wrote in count too, so replying in one of its threads also reaches it.

The agent writes as `urn:agent:pi` ("Pi"), and the watch follows that id, so its own replies never wake it again. Set `ANNOX_AUTHOR` (and `ANNOX_AUTHOR_NAME`) before starting pi to use another id.

Whoever can add annotations to the workspace can address the agent this way, collaborators through git or a sync hub included, so only use it in workspaces whose collaborators you trust with your session.
