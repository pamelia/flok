# MCP in Flok

Flok can load external tools through the Model Context Protocol (MCP).

Today it supports:

- local stdio MCP servers
- remote HTTP MCP servers
- remote bearer-token auth through an environment variable
- namespaced MCP tool registration alongside built-in tools

It does not require recompiling `flok` to add a new MCP server.

## Config Location

Flok stores user config at the platform config path:

- Linux: `~/.config/flok/flok.toml`
- macOS: `~/Library/Application Support/flok/flok.toml`
- Windows: `{RoamingAppData}\flok\flok.toml`

MCP server entries live under `[mcp_servers.<name>]`.

## Add a Remote MCP Server

Example: GitHub MCP.

```bash
export GITHUB_PAT_TOKEN=ghp_...
flok mcp add github --url https://api.githubcopilot.com/mcp/ --bearer-token-env-var GITHUB_PAT_TOKEN
```

This writes config equivalent to:

```toml
[mcp_servers.github]
url = "https://api.githubcopilot.com/mcp/"
bearer_token_env_var = "GITHUB_PAT_TOKEN"
```

## Add a Local Stdio MCP Server

```bash
flok mcp add filesystem --command npx --arg -y --arg @modelcontextprotocol/server-filesystem --arg .
```

This writes config equivalent to:

```toml
[mcp_servers.filesystem]
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
```

You can also set:

- `--cwd <path>` for stdio servers
- `--timeout-seconds <n>` for either transport
- `--disabled` to keep the config entry without loading it

## TUI Commands

Inside the TUI:

- `/mcp`
- `/mcp list`
- `/mcp status`
- `/mcp add <name> --url <url>`
- `/mcp add <name> --command <command> [--arg ...]`

`/mcp list` shows configured servers from your user config.

## How MCP Tools Appear

MCP tools are registered under namespaced names:

- `github_get_me`
- `github_search_repositories`
- `filesystem_read_file`

This prevents collisions with built-in tools like `read`, `write`, or `bash`.

## Tool Pickup Behavior

Flok registers configured MCP servers during startup.

That means:

- adding or changing an MCP server updates config immediately
- new or changed MCP tools are picked up the next time `flok` starts
- listing configured MCP servers does not mean those tools were newly loaded in the current session

## Current Limitations

Current MCP support is useful, but not complete. The main limitations are:

- MCP server discovery happens at startup, not as a live hot-reload of the active tool registry
- remote auth currently supports bearer token env vars, not OAuth flows
- remote MCP support covers request/response flows used for `initialize`, `tools/list`, and `tools/call`
- server prompts, resources, sampling, and richer server-initiated request handling are not implemented yet

## Troubleshooting

If `/mcp list` shows your server but the tools are unavailable:

1. Confirm the config entry looks correct.
2. Confirm any bearer token env var is exported in the shell that launches `flok`.
3. Restart `flok` so it picks up newly configured MCP servers.

If a remote server needs auth and the token env var is missing or empty, flok will fail that MCP server cleanly without crashing the app.
