// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppError {
    #[error("Node.js runtime not found")]
    NodeNotFound,
    #[error("Agent bridge process failed to start")]
    BridgeSpawnFailed,
    #[error("Agent bridge initialization failed")]
    BridgeInitializationFailed,
    #[error("Agent bridge stdout closed")]
    BridgeStdoutClosed,
    #[error("Agent SDK bridge failed")]
    BridgeSdkFailure,
    #[error("Agent bridge initialization timed out")]
    BridgeTimeout,
    #[error("Agent bridge process failed")]
    AdapterCrashed,
    #[error("Agent bridge connection failed")]
    ConnectionFailed,
    #[error("Session not found")]
    SessionNotFound,
    #[error("Authentication required")]
    AuthRequired,
}

impl AppError {
    pub const NODE_NOT_FOUND_EXIT_CODE: i32 = 20;
    pub const ADAPTER_CRASHED_EXIT_CODE: i32 = 21;
    pub const CONNECTION_FAILED_EXIT_CODE: i32 = 22;
    pub const SESSION_NOT_FOUND_EXIT_CODE: i32 = 23;
    pub const AUTH_REQUIRED_EXIT_CODE: i32 = 24;

    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::NodeNotFound => Self::NODE_NOT_FOUND_EXIT_CODE,
            Self::BridgeSpawnFailed | Self::AdapterCrashed => Self::ADAPTER_CRASHED_EXIT_CODE,
            Self::BridgeInitializationFailed
            | Self::BridgeStdoutClosed
            | Self::BridgeSdkFailure
            | Self::BridgeTimeout
            | Self::ConnectionFailed => Self::CONNECTION_FAILED_EXIT_CODE,
            Self::SessionNotFound => Self::SESSION_NOT_FOUND_EXIT_CODE,
            Self::AuthRequired => Self::AUTH_REQUIRED_EXIT_CODE,
        }
    }

    #[must_use]
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::NodeNotFound => {
                "Node.js runtime not found. Install Node.js and ensure `node` is on PATH."
            }
            Self::BridgeSpawnFailed => {
                "Agent bridge process failed to start. Run `claude-rs doctor --strict` to inspect the runtime and bridge script."
            }
            Self::BridgeInitializationFailed => {
                "Agent bridge failed during initialization. Run `claude-rs logs --bundle --yes` to collect diagnostics."
            }
            Self::BridgeStdoutClosed => {
                "Agent bridge exited before completing the protocol. Run `claude-rs logs --latest` for details."
            }
            Self::BridgeSdkFailure => {
                "Agent SDK bridge reported a failure. Run `claude-rs logs --bundle --yes` to collect diagnostics."
            }
            Self::BridgeTimeout => {
                "Agent bridge did not initialize before the timeout. Run `claude-rs doctor --strict` and retry with bridge diagnostics enabled."
            }
            Self::AdapterCrashed => "Agent bridge process crashed or failed to start.",
            Self::ConnectionFailed => {
                "Failed to establish or maintain the Agent SDK bridge connection."
            }
            Self::SessionNotFound => "The requested session was not found.",
            Self::AuthRequired => {
                "Authentication required. Type /login to authenticate, or run `claude auth login` in a terminal."
            }
        }
    }

    #[must_use]
    pub fn report_title(&self) -> &'static str {
        match self {
            Self::NodeNotFound => "Environment problem",
            Self::BridgeSpawnFailed | Self::AdapterCrashed => "Bridge spawn failure",
            Self::BridgeInitializationFailed => "Bridge initialization failure",
            Self::BridgeStdoutClosed => "Bridge process exited",
            Self::BridgeSdkFailure | Self::ConnectionFailed => "Bridge communication failure",
            Self::BridgeTimeout => "Bridge initialization timeout",
            Self::SessionNotFound => "Session not found",
            Self::AuthRequired => "Authentication required",
        }
    }

    #[must_use]
    pub fn category_tag(&self) -> &'static str {
        match self {
            Self::NodeNotFound => "environment",
            Self::BridgeSpawnFailed | Self::AdapterCrashed => "bridge_spawn",
            Self::BridgeInitializationFailed => "bridge_initialization",
            Self::BridgeStdoutClosed => "bridge_stdout_closed",
            Self::BridgeSdkFailure | Self::ConnectionFailed => "bridge_sdk_failure",
            Self::BridgeTimeout => "bridge_timeout",
            Self::SessionNotFound => "session",
            Self::AuthRequired => "auth",
        }
    }

    #[must_use]
    pub fn recommended_command(&self) -> &'static str {
        match self {
            Self::NodeNotFound
            | Self::BridgeSpawnFailed
            | Self::AdapterCrashed
            | Self::BridgeTimeout => "claude-rs doctor --strict",
            Self::BridgeInitializationFailed
            | Self::BridgeStdoutClosed
            | Self::BridgeSdkFailure
            | Self::ConnectionFailed => "claude-rs logs --bundle --yes",
            Self::SessionNotFound => "claude-rs resume",
            Self::AuthRequired => "claude auth login",
        }
    }
}
