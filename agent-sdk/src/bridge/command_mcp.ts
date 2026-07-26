import type { BridgeCommand } from "../types.js";
import { slashError } from "./events.js";
import {
  handleMcpAuthenticateCommand,
  handleMcpClearAuthCommand,
  handleMcpOauthCallbackUrlCommand,
  handleMcpReconnectCommand,
  handleMcpSetServersCommand,
  handleMcpStatusCommand,
  handleMcpToggleCommand,
} from "./mcp.js";
import { sessionById } from "./session_lifecycle.js";

type McpCommand = Extract<
  BridgeCommand,
  {
    command:
      | "mcp_status"
      | "mcp_reconnect"
      | "mcp_toggle"
      | "mcp_set_servers"
      | "mcp_authenticate"
      | "mcp_clear_auth"
      | "mcp_oauth_callback_url";
  }
>;

export async function handleMcpCommand(
  command: McpCommand,
  requestId?: string,
): Promise<void> {
  const session = sessionById(command.session_id);
  if (!session) {
    slashError(command.session_id, `unknown session: ${command.session_id}`, requestId);
    return;
  }
  switch (command.command) {
    case "mcp_status":
      await handleMcpStatusCommand(session, requestId);
      return;
    case "mcp_reconnect":
      await handleMcpReconnectCommand(session, command, requestId);
      return;
    case "mcp_toggle":
      await handleMcpToggleCommand(session, command, requestId);
      return;
    case "mcp_set_servers":
      await handleMcpSetServersCommand(session, command, requestId);
      return;
    case "mcp_authenticate":
      await handleMcpAuthenticateCommand(session, command, requestId);
      return;
    case "mcp_clear_auth":
      await handleMcpClearAuthCommand(session, command, requestId);
      return;
    case "mcp_oauth_callback_url":
      await handleMcpOauthCallbackUrlCommand(session, command, requestId);
  }
}
