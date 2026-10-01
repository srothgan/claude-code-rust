import type { Query } from "@anthropic-ai/claude-agent-sdk";
import type {
  BridgeEvent,
  Json,
  SideQuestionMetadata,
} from "../types.js";
import { writeEvent } from "./events.js";
import { bridgeLogger, LOG_TARGETS } from "./logger.js";

type SideQuestionItem = {
  btwId: string;
  question: string;
};

type SideQuestionResult = {
  response: string;
  synthetic: boolean;
  refusalFallback?: {
    originalModel: string;
    fallbackModel: string;
    content: Json;
  };
};

type SideQuestionEventWriter = (event: BridgeEvent) => void;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isJson(value: unknown): value is Json {
  if (
    value === null ||
    typeof value === "string" ||
    typeof value === "number" ||
    typeof value === "boolean"
  ) {
    return true;
  }
  if (Array.isArray(value)) {
    return value.every(isJson);
  }
  return isRecord(value) && Object.values(value).every(isJson);
}

function normalizeSideQuestionResult(value: unknown): SideQuestionResult {
  if (value === null) {
    throw new Error("Claude returned no side-question answer");
  }
  if (!isRecord(value)) {
    throw new Error("SDK returned a malformed side-question result");
  }
  if (typeof value.response !== "string" || value.response.length === 0) {
    throw new Error("SDK side-question result omitted its response");
  }
  if (typeof value.synthetic !== "boolean") {
    throw new Error("SDK side-question result omitted its synthetic flag");
  }

  let refusalFallback: SideQuestionResult["refusalFallback"];
  if (value.refusalFallback !== undefined) {
    if (!isRecord(value.refusalFallback)) {
      throw new Error("SDK returned malformed side-question refusal metadata");
    }
    const { originalModel, fallbackModel, content } = value.refusalFallback;
    if (
      typeof originalModel !== "string" ||
      typeof fallbackModel !== "string" ||
      !isJson(content)
    ) {
      throw new Error("SDK returned malformed side-question refusal metadata");
    }
    refusalFallback = { originalModel, fallbackModel, content };
  }

  return {
    response: value.response,
    synthetic: value.synthetic,
    ...(refusalFallback ? { refusalFallback } : {}),
  };
}

function metadataFromResult(result: SideQuestionResult): SideQuestionMetadata {
  return {
    synthetic: result.synthetic,
    ...(result.refusalFallback
      ? {
          refusal_fallback: {
            original_model: result.refusalFallback.originalModel,
            fallback_model: result.refusalFallback.fallbackModel,
            content: result.refusalFallback.content,
          },
        }
      : {}),
  };
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

// Rust owns admission and FIFO ordering; this adapter never retains waiting work.
export class SideQuestionAdapter {
  private activeController: AbortController | undefined;
  private closed = false;

  constructor(
    private readonly sessionId: string,
    private readonly query: Query,
    private readonly emit: SideQuestionEventWriter = writeEvent,
  ) {}

  close(): void {
    this.closed = true;
    this.activeController?.abort();
  }

  async dispatch(item: SideQuestionItem): Promise<void> {
    if (this.closed) {
      return;
    }
    if (this.activeController) {
      this.emitFailure(item, "Bridge protocol error: a side question is already active");
      return;
    }
    const method = (this.query as { askSideQuestion?: unknown })
      .askSideQuestion;
    if (typeof method !== "function") {
      this.emitFailure(
        item,
        "The installed Agent SDK does not support side questions",
      );
      return;
    }

    const controller = new AbortController();
    this.activeController = controller;
    let event: BridgeEvent;
    try {
      const raw = await method.call(this.query, item.question, {
        signal: controller.signal,
      });
      const result = normalizeSideQuestionResult(raw);
      event = {
        event: "btw_result",
        session_id: this.sessionId,
        btw_id: item.btwId,
        question: item.question,
        answer: result.response,
        metadata: metadataFromResult(result),
      };
    } catch (error) {
      event = this.failureEvent(item, errorMessage(error));
    } finally {
      if (this.activeController === controller) {
        this.activeController = undefined;
      }
    }
    // Release the active call before publishing the terminal event to the host.
    if (!this.closed) {
      this.emit(event);
    }
  }

  private emitFailure(item: SideQuestionItem, error: string): void {
    this.emit(this.failureEvent(item, error));
  }

  private failureEvent(item: SideQuestionItem, error: string): BridgeEvent {
    bridgeLogger.warn({
      target: LOG_TARGETS.BRIDGE_PROTOCOL,
      eventName: "side_question_failed",
      message: "side question failed",
      outcome: "failure",
      sessionId: this.sessionId,
      fields: { btw_id: item.btwId, error_message: error },
    });
    return {
      event: "btw_failed",
      session_id: this.sessionId,
      btw_id: item.btwId,
      question: item.question,
      error,
    };
  }
}

const adapters = new Map<
  string,
  { query: Query; adapter: SideQuestionAdapter }
>();

export function dispatchSideQuestion(
  sessionId: string,
  query: Query,
  item: SideQuestionItem,
): void {
  const existing = adapters.get(sessionId);
  if (existing && existing.query !== query) {
    existing.adapter.close();
    adapters.delete(sessionId);
  }
  let current = adapters.get(sessionId);
  if (!current) {
    current = {
      query,
      adapter: new SideQuestionAdapter(sessionId, query),
    };
    adapters.set(sessionId, current);
  }
  void current.adapter.dispatch(item);
}

export function closeSideQuestions(sessionId: string, query: Query): void {
  const current = adapters.get(sessionId);
  if (!current || current.query !== query) {
    return;
  }
  current.adapter.close();
  adapters.delete(sessionId);
}
