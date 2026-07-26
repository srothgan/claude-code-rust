import type { BridgeCommand } from "../types.js";
import {
  handleElicitationResponse,
  handlePermissionResponse,
  handleQuestionResponse,
  handleUserDialogResponse,
} from "./session_lifecycle.js";

type InteractionCommand = Extract<
  BridgeCommand,
  {
    command:
      | "permission_response"
      | "question_response"
      | "user_dialog_response"
      | "elicitation_response";
  }
>;

export function handleInteractionCommand(command: InteractionCommand): void {
  switch (command.command) {
    case "permission_response":
      handlePermissionResponse(command);
      return;
    case "question_response":
      handleQuestionResponse(command);
      return;
    case "user_dialog_response":
      handleUserDialogResponse(command);
      return;
    case "elicitation_response":
      handleElicitationResponse(command);
  }
}
