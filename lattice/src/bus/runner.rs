use std::sync::Arc;

use crate::agent::memory::Memory;
use crate::agent::Agent;
use tracing::warn;

use crate::bus::json_output::strip_markdown_fence;
use crate::bus::profile::AgentProfile;

// ---------------------------------------------------------------------------
// AgentRunner — wires AgentProfile + Agent
// ---------------------------------------------------------------------------

/// Max retries for JSON schema validation failures.
const MAX_SCHEMA_RETRIES: u32 = 2;

/// A runner that executes an agent per its profile.
pub struct AgentRunner {
    pub profile: AgentProfile,
    pub agent: Agent,
    pub shared_memory: Option<Arc<dyn Memory>>,
}

impl AgentRunner {
    /// Create a runner from a profile and resolved agent.
    pub fn from_profile(profile: AgentProfile, agent: Agent) -> Self {
        Self {
            profile,
            agent,
            shared_memory: None,
        }
    }

    /// Attach shared memory for implicit recall before each run.
    pub fn with_memory(mut self, memory: Arc<dyn Memory>) -> Self {
        self.shared_memory = Some(memory);
        self
    }

    /// Access the inner Agent for bus registration or direct manipulation.
    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    pub fn agent_mut(&mut self) -> &mut Agent {
        &mut self.agent
    }

    /// Run the agent with the given input. Returns the JSON output.
    ///
    /// If `output_schema` is configured on the profile, the output is validated
    /// against the JSON Schema and retried with format hints on failure.
    ///
    /// Handoff routing is NOT done here — the caller (Pipeline) evaluates
    /// `handoff_rules` against the returned output to determine the next agent.
    pub async fn run(
        &mut self,
        input: &str,
        max_turns: u32,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let schema = match self.profile.handoff.output_schema.as_ref() {
            Some(s) => match serde_json::from_str::<serde_json::Value>(s) {
                Ok(schema_json) => match jsonschema::validator_for(&schema_json) {
                    Ok(validator) => Some((schema_json, validator)),
                    Err(e) => {
                        return Err(format!(
                            "output_schema is not a valid JSON Schema for {}: {e}",
                            self.profile.agent.name
                        )
                        .into());
                    }
                },
                Err(e) => {
                    return Err(format!(
                        "output_schema is not valid JSON for {}: {e}",
                        self.profile.agent.name
                    )
                    .into());
                }
            },
            None => None,
        };

        let mut output = self.run_once(input, max_turns).await?;

        // JSON Schema validation + retry loop
        if let Some((ref schema_json, ref validator)) = schema {
            let mut schema_valid = false;
            for retry in 0..MAX_SCHEMA_RETRIES {
                let mut errors = validator.iter_errors(&output);
                let first_error = errors.next();

                if first_error.is_none() {
                    schema_valid = true;
                    break; // Valid ✓
                }

                let error_messages: Vec<String> = std::iter::once(first_error.unwrap())
                    .chain(errors)
                    .take(3)
                    .map(|e| format!("- {}", e))
                    .collect();

                warn!(
                    "Output validation failed for {} (attempt {}/{}):\n{}",
                    self.profile.agent.name,
                    retry + 1,
                    MAX_SCHEMA_RETRIES,
                    error_messages.join("\n")
                );

                let correction_hint = format!(
                    "{}\n\nYour previous response did not match the required JSON format. \
                     Errors:\n{}\n\nExpected schema:\n{}\n\nPlease correct your response. \
                     Return ONLY valid JSON that matches the schema, no markdown.",
                    input,
                    error_messages.join("\n"),
                    serde_json::to_string_pretty(schema_json).unwrap_or_default()
                );

                output = self.run_once(&correction_hint, max_turns).await?;
            }

            if !schema_valid {
                return Err(format!(
                    "Agent '{}' output still fails schema validation after {} retries",
                    self.profile.agent.name, MAX_SCHEMA_RETRIES
                )
                .into());
            }
        }

        Ok(output)
    }

    /// Check if agent output content looks like an error message rather than
    /// a genuine attempt to produce structured output.
    fn looks_like_error(content: &str) -> bool {
        let lower = content.trim().to_ascii_lowercase();
        let trimmed = content.trim();
        const CASE_SENSITIVE_PREFIXES: &[&str] = &[
            "Error:",
            "error:",
            "Provider error:",
            "API error:",
            "Authentication error:",
            "Rate limit",
            "401",
            "403",
            "429",
            "500",
            "502",
            "503",
            "504",
        ];
        const LOWER_PREFIXES: &[&str] = &[
            "rate limit",
            "authentication",
            "unauthorized",
            "forbidden",
            "api key",
            "invalid api key",
            "invalid request",
            "bad request",
            "server error",
            "internal server error",
            "service unavailable",
            "temporarily unavailable",
            "timeout",
            "request timeout",
            "connection refused",
            "connection reset",
            "connection closed",
            "network error",
            "dns",
            "tls",
            "certificate",
        ];
        const LOWER_CONTAINS: &[&str] = &[
            "\"error\"",
            "context_length_exceeded",
            "rate_limit",
            "invalid_api_key",
        ];

        CASE_SENSITIVE_PREFIXES
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
            || LOWER_PREFIXES
                .iter()
                .any(|prefix| lower.starts_with(prefix))
            || LOWER_CONTAINS.iter().any(|needle| lower.contains(needle))
    }

    /// Run the agent once and parse the output as JSON.
    async fn run_once(
        &mut self,
        input: &str,
        max_turns: u32,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let events = self.agent.run(input, max_turns).await;
        // Propagate error events immediately
        for event in &events {
            if let crate::agent::LoopEvent::Error { message, .. } = event {
                return Err(format!("Agent error: {}", message).into());
            }
        }
        let mut content = String::new();
        for event in &events {
            if let crate::agent::LoopEvent::Token { text } = event {
                content.push_str(text);
            }
        }

        // Empty output + output_schema → error (tool-only completion needs schema response)
        if content.is_empty() && self.profile.handoff.output_schema.is_some() {
            return Err(
                "Agent produced empty output; output_schema requires structured response".into(),
            );
        }

        // If the content looks like a provider/auth error, don't silently fall
        // back to JSON wrapping — return an error so the pipeline can decide
        // whether to retry based on the error type.
        if Self::looks_like_error(&content) {
            return Err(format!("Agent returned error-like output: {}", content).into());
        }

        let output: serde_json::Value = match serde_json::from_str(strip_markdown_fence(&content)) {
            Ok(v) => v,
            Err(_) if self.profile.handoff.output_schema.is_some() => {
                return Err(
                    "Agent output is not valid JSON; output_schema requires structured output"
                        .into(),
                );
            }
            Err(_) => serde_json::json!({"_raw_text": true, "content": content}),
        };

        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::AgentRunner;

    #[test]
    fn detects_provider_and_transport_error_text() {
        assert!(AgentRunner::looks_like_error(
            r#"{"error":{"message":"invalid_api_key"}}"#
        ));
        assert!(AgentRunner::looks_like_error(
            "503 service unavailable: upstream overloaded"
        ));
        assert!(AgentRunner::looks_like_error(
            "Network error: connection reset by peer"
        ));
        assert!(AgentRunner::looks_like_error(
            "context_length_exceeded: prompt too long"
        ));
    }

    #[test]
    fn does_not_treat_normal_text_as_error() {
        assert!(!AgentRunner::looks_like_error(
            "Here is the requested implementation summary."
        ));
    }
}
