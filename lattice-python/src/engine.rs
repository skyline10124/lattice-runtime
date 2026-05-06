use std::collections::HashMap;
use std::sync::mpsc;
use std::sync::RwLock;
use std::sync::{LazyLock, Mutex};

use lattice_core::catalog::{CredentialStatus, ResolvedModel};
use lattice_core::provider::ChatResponse;
use lattice_core::router::ModelRouter;
use lattice_core::types::{FunctionCall, Message, Role, ToolCall, ToolDefinition};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyAny, PyDict, PyList};
use pyo3::Py;

use crate::errors::convert_core_error;

// ---------------------------------------------------------------------------
// Shared tokio runtime for bridging sync → async in PyO3 methods
//
// This is intentional and correct: PyO3 methods are synchronous (Python
// calling convention), but lattice-core functions are async. We need a
// runtime bridge. Unlike the removed SYNC_RT/BUS_RT in lattice-harness
// (which caused nested-runtime panics), this runtime is only used at the
// outermost sync boundary — it never calls Pipeline::run() or
// MicroAgent::spawn(), which are now fully async.
//
// We use spawn + oneshot channel instead of block_in_place, which avoids
// the single-threaded runtime panic: block_in_place requires the current
// runtime to be multi-threaded (it needs other worker threads to continue
// processing). With spawn + blocking_recv, SHARED_RUNTIME processes the
// task on its own threads regardless of the calling context. Works in
// all scenarios: no runtime, multi-threaded runtime, single-threaded runtime.
// ---------------------------------------------------------------------------

static SHARED_RUNTIME: LazyLock<tokio::runtime::Runtime> = LazyLock::new(|| {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create shared tokio runtime for Python bindings")
});

/// Run an async future synchronously via spawn + oneshot channel.
///
/// Always spawns on SHARED_RUNTIME and blocks the calling thread until
/// completion. This avoids `block_in_place` which panics inside a
/// single-threaded tokio runtime, and avoids `block_on` which panics
/// when called from within any tokio context ("Cannot start a runtime
/// from within a runtime").
fn run_async<F, T>(f: F) -> PyResult<T>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    SHARED_RUNTIME.spawn(async move {
        let result = f.await;
        tx.send(result).ok();
    });
    rx.blocking_recv().map_err(|_| {
        PyRuntimeError::new_err("Async task was cancelled or panicked before completion")
    })
}

// ---------------------------------------------------------------------------
// Conversion helpers: Python → Rust → Python
// ---------------------------------------------------------------------------

/// Convert a list of Python message dicts into Rust `Message` values.
fn messages_from_py(
    py: Python<'_>,
    messages: Vec<HashMap<String, Py<PyAny>>>,
) -> PyResult<Vec<Message>> {
    let mut result = Vec::with_capacity(messages.len());
    for m in messages {
        let role_str: String = m
            .get("role")
            .ok_or_else(|| PyValueError::new_err("Each message must have a 'role' field"))?
            .extract(py)?;

        let role = match role_str.to_lowercase().as_str() {
            "system" => Role::System,
            "user" => Role::User,
            "assistant" => Role::Assistant,
            "tool" => Role::Tool,
            other => {
                return Err(PyValueError::new_err(format!(
                    "Invalid role '{}'. Must be one of: system, user, assistant, tool",
                    other
                )));
            }
        };

        let content: String = m
            .get("content")
            .map(|v| v.extract::<String>(py).unwrap_or_default())
            .unwrap_or_default();

        let reasoning_content = match m.get("reasoning_content") {
            Some(value) => Some(value.extract::<String>(py)?),
            None => None,
        };
        let tool_call_id = match m.get("tool_call_id") {
            Some(value) => Some(value.extract::<String>(py)?),
            None => None,
        };
        let name = match m.get("name") {
            Some(value) => Some(value.extract::<String>(py)?),
            None => None,
        };
        let tool_calls = match m.get("tool_calls") {
            Some(value) => Some(tool_calls_from_py(py, value)?),
            None => None,
        };

        let mut msg = Message::new(role, content, tool_calls, tool_call_id, name);
        msg.reasoning_content = reasoning_content;
        result.push(msg);
    }
    Ok(result)
}

fn tool_calls_from_py(py: Python<'_>, value: &Py<PyAny>) -> PyResult<Vec<ToolCall>> {
    let calls = value.bind(py).cast::<PyList>()?;
    let mut result = Vec::with_capacity(calls.len());
    for item in calls.iter() {
        let dict = item.cast::<PyDict>()?;
        let id: String = dict
            .get_item("id")?
            .ok_or_else(|| PyValueError::new_err("Each tool_call must have an 'id' field"))?
            .extract()?;
        let function = dict
            .get_item("function")?
            .ok_or_else(|| PyValueError::new_err("Each tool_call must have a 'function' field"))?;
        let function = function.cast::<PyDict>()?;
        let name: String = function
            .get_item("name")?
            .ok_or_else(|| {
                PyValueError::new_err("Each tool_call.function must have a 'name' field")
            })?
            .extract()?;
        let arguments: String = function
            .get_item("arguments")?
            .ok_or_else(|| {
                PyValueError::new_err("Each tool_call.function must have an 'arguments' field")
            })?
            .extract()?;
        result.push(ToolCall {
            id,
            function: FunctionCall { name, arguments },
        });
    }
    Ok(result)
}

/// Convert a list of Python tool-definition dicts into Rust `ToolDefinition` values.
fn tools_from_py(
    py: Python<'_>,
    tools: Vec<HashMap<String, Py<PyAny>>>,
) -> PyResult<Vec<ToolDefinition>> {
    let json_mod = py.import("json")?;
    let mut result = Vec::with_capacity(tools.len());

    for t in tools {
        let name: String = t
            .get("name")
            .ok_or_else(|| PyValueError::new_err("Each tool must have a 'name' field"))?
            .extract(py)?;

        let description: String = t
            .get("description")
            .map(|v| v.extract::<String>(py).unwrap_or_default())
            .unwrap_or_default();

        let parameters = match t.get("parameters") {
            Some(params) => {
                let json_str: String = json_mod.call_method1("dumps", (params,))?.extract()?;
                serde_json::from_str(&json_str).map_err(|e| {
                    PyValueError::new_err(format!(
                        "Invalid JSON schema in tool '{}' parameters: {}",
                        name, e
                    ))
                })?
            }
            None => serde_json::Value::Null,
        };

        result.push(ToolDefinition::new(name, description, parameters));
    }
    Ok(result)
}

/// Convert a Rust `ChatResponse` into a Python dict.
fn chat_response_to_py(py: Python<'_>, response: ChatResponse) -> PyResult<Py<PyAny>> {
    let dict = PyDict::new(py);

    if let Some(ref content) = response.content {
        dict.set_item("content", content)?;
    }
    if let Some(ref reasoning) = response.reasoning_content {
        dict.set_item("reasoning_content", reasoning)?;
    }
    dict.set_item("finish_reason", &response.finish_reason)?;
    dict.set_item("model", &response.model)?;

    if let Some(ref usage) = response.usage {
        let usage_dict = PyDict::new(py);
        usage_dict.set_item("prompt_tokens", usage.prompt_tokens)?;
        usage_dict.set_item("completion_tokens", usage.completion_tokens)?;
        usage_dict.set_item("total_tokens", usage.total_tokens)?;
        dict.set_item("usage", usage_dict)?;
    }

    if let Some(ref tool_calls) = response.tool_calls {
        let tc_list = PyList::empty(py);
        for tc in tool_calls {
            let tc_dict = PyDict::new(py);
            tc_dict.set_item("id", &tc.id)?;
            let fn_dict = PyDict::new(py);
            fn_dict.set_item("name", &tc.function.name)?;
            fn_dict.set_item("arguments", &tc.function.arguments)?;
            tc_dict.set_item("function", fn_dict)?;
            tc_list.append(tc_dict)?;
        }
        dict.set_item("tool_calls", tc_list)?;
    }

    Ok(dict.into())
}

/// Python-facing model resolver.
#[pyclass]
pub struct LatticeEngine {
    router: RwLock<ModelRouter>,
}

#[pymethods]
impl LatticeEngine {
    #[new]
    #[pyo3(signature = (credentials=None))]
    pub fn new(credentials: Option<HashMap<String, String>>) -> Self {
        let router = match credentials {
            Some(map) => ModelRouter::with_credentials(map),
            None => ModelRouter::new(),
        };
        Self {
            router: RwLock::new(router),
        }
    }

    /// Resolve a model name to connection details.
    /// Rejects non-localhost HTTP base URLs for security.
    #[pyo3(signature = (model, provider_override=None))]
    pub fn resolve_model(
        &self,
        model: &str,
        provider_override: Option<&str>,
    ) -> PyResult<PyResolvedModel> {
        let router = self.router.read().unwrap_or_else(|e| {
            tracing::warn!("RwLock poisoned in LatticeEngine::resolve_model, recovering");
            e.into_inner()
        });

        let resolved = router
            .resolve(model, provider_override)
            .map_err(convert_core_error)?;

        Ok(PyResolvedModel { inner: resolved })
    }

    /// List all canonical model IDs.
    pub fn list_models(&self) -> Vec<String> {
        let router = self.router.read().unwrap_or_else(|e| {
            tracing::warn!("RwLock poisoned in LatticeEngine::list_models, recovering");
            e.into_inner()
        });
        router.list_models()
    }

    /// List models with valid credentials.
    pub fn list_authenticated_models(&self) -> Vec<String> {
        let router = self.router.read().unwrap_or_else(|e| {
            tracing::warn!(
                "RwLock poisoned in LatticeEngine::list_authenticated_models, recovering"
            );
            e.into_inner()
        });
        router.list_authenticated_models()
    }

    /// Send messages to a resolved model and return the complete response.
    ///
    /// Args:
    ///     resolved: A PyResolvedModel from `resolve_model()`.
    ///     messages: List of dicts with `{"role": ..., "content": ...}`.
    ///               Roles: "system", "user", "assistant", "tool".
    ///     tools: Optional list of tool definition dicts, each with
    ///            `{"name": ..., "description": ..., "parameters": {...}}`.
    ///
    /// Returns: A dict with keys `content`, `finish_reason`, `model`,
    ///          and optionally `usage`, `tool_calls`, `reasoning_content`.
    #[pyo3(signature = (resolved, messages, tools=None))]
    fn chat_complete(
        &self,
        py: Python<'_>,
        resolved: &PyResolvedModel,
        messages: Vec<HashMap<String, Py<PyAny>>>,
        tools: Option<Vec<HashMap<String, Py<PyAny>>>>,
    ) -> PyResult<Py<PyAny>> {
        let msgs = messages_from_py(py, messages)?;
        let tool_defs = match tools {
            Some(t) => tools_from_py(py, t)?,
            None => Vec::new(),
        };
        let resolved_inner = resolved.inner.clone();

        let response = run_async(async move {
            lattice_core::chat_complete(&resolved_inner, &msgs, &tool_defs).await
        })?
        .map_err(convert_core_error)?;

        chat_response_to_py(py, response)
    }

    /// Send messages to a resolved model and return a synchronous iterator of
    /// stream event dicts.  Each dict has the keys:
    ///
    /// - `"type"`: one of `"token"`, `"reasoning"`, `"tool_call_start"`,
    ///   `"tool_call_delta"`, `"tool_call_end"`, `"done"`, `"error"`
    /// - `"content"`: the string payload (present for `token`, `reasoning`, `error`)
    /// - `"id"`: tool call id (present for tool_call events)
    /// - `"name"`: tool call name (present for `tool_call_start`)
    /// - `"arguments_delta"`: partial JSON arguments (present for `tool_call_delta`)
    /// - `"finish_reason"`: reason string (present for `done`)
    ///
    /// This is a sync Python iterator over the Rust async stream; events are
    /// pushed incrementally through a channel as the provider sends them.
    #[pyo3(signature = (resolved, messages, tools=None))]
    fn stream_chat(
        &self,
        py: Python<'_>,
        resolved: &PyResolvedModel,
        messages: Vec<HashMap<String, Py<PyAny>>>,
        tools: Option<Vec<HashMap<String, Py<PyAny>>>>,
    ) -> PyResult<StreamIterator> {
        let msgs = messages_from_py(py, messages)?;
        let tool_defs = match tools {
            Some(t) => tools_from_py(py, t)?,
            None => Vec::new(),
        };
        let resolved_inner = resolved.inner.clone();

        let (tx, rx) = mpsc::sync_channel(32);
        SHARED_RUNTIME.spawn(async move {
            let stream = match lattice_core::chat(&resolved_inner, &msgs, &tool_defs).await {
                Ok(s) => s,
                Err(e) => {
                    let _ = tx.send(serde_json::json!({"type": "error", "content": e.to_string()}));
                    return;
                }
            };

            use futures::StreamExt;
            let mut s = stream;
            while let Some(event) = s.next().await {
                let json_val = match event {
                    lattice_core::StreamEvent::Token { content } => {
                        serde_json::json!({"type": "token", "content": content})
                    }
                    lattice_core::StreamEvent::Reasoning { content } => {
                        serde_json::json!({"type": "reasoning", "content": content})
                    }
                    lattice_core::StreamEvent::ToolCallStart { id, name } => {
                        serde_json::json!({"type": "tool_call_start", "id": id, "name": name})
                    }
                    lattice_core::StreamEvent::ToolCallDelta {
                        id,
                        arguments_delta,
                    } => {
                        serde_json::json!({"type": "tool_call_delta", "id": id, "arguments_delta": arguments_delta})
                    }
                    lattice_core::StreamEvent::ToolCallEnd { id } => {
                        serde_json::json!({"type": "tool_call_end", "id": id})
                    }
                    lattice_core::StreamEvent::Done {
                        finish_reason,
                        usage,
                    } => {
                        let mut d =
                            serde_json::json!({"type": "done", "finish_reason": finish_reason});
                        if let Some(ref u) = usage {
                            d["usage"] = serde_json::json!({
                                "prompt_tokens": u.prompt_tokens,
                                "completion_tokens": u.completion_tokens,
                                "total_tokens": u.total_tokens,
                            });
                        }
                        d
                    }
                    lattice_core::StreamEvent::Error { message } => {
                        serde_json::json!({"type": "error", "content": message})
                    }
                };
                if tx.send(json_val).is_err() {
                    break;
                }
            }
        });

        let _ = py;
        Ok(StreamIterator { rx: Mutex::new(rx) })
    }
}

/// Python iterator that yields stream events as dicts.
///
/// Wraps the async [`StreamEvent`] stream from `stream_chat()` and
/// materialises events synchronously on each `__next__()` call.
#[pyclass]
pub struct StreamIterator {
    rx: Mutex<mpsc::Receiver<serde_json::Value>>,
}

#[pymethods]
impl StreamIterator {
    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(slf: PyRef<'_, Self>, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        let rx = slf
            .rx
            .lock()
            .map_err(|_| PyRuntimeError::new_err("stream receiver lock poisoned"))?;
        match rx.recv() {
            Ok(value) => Ok(Some(json_value_to_py(py, value)?)),
            Err(_) => Ok(None),
        }
    }
}

fn json_value_to_py(py: Python<'_>, value: serde_json::Value) -> PyResult<Py<PyAny>> {
    let json_mod = py
        .import("json")
        .map_err(|e| PyValueError::new_err(format!("Failed to import json: {e}")))?;
    let json_str =
        serde_json::to_string(&value).map_err(|e| PyValueError::new_err(e.to_string()))?;
    Ok(json_mod
        .call_method1("loads", (json_str,))
        .map_err(|e| PyValueError::new_err(format!("JSON decode failed: {e}")))?
        .into())
}

/// Python-facing resolved model (read-only).
#[pyclass(skip_from_py_object)]
#[derive(Clone)]
pub struct PyResolvedModel {
    inner: ResolvedModel,
}

#[pymethods]
impl PyResolvedModel {
    #[getter]
    pub fn canonical_id(&self) -> &str {
        &self.inner.canonical_id
    }

    #[getter]
    pub fn provider(&self) -> &str {
        &self.inner.provider
    }

    #[getter]
    pub fn api_model_id(&self) -> &str {
        &self.inner.api_model_id
    }

    #[getter]
    pub fn context_length(&self) -> u32 {
        self.inner.context_length
    }

    #[getter]
    pub fn credential_status(&self) -> String {
        match self.inner.credential_status {
            CredentialStatus::Present => "present".to_string(),
            CredentialStatus::NotRequired => "not_required".to_string(),
            CredentialStatus::Missing => "missing".to_string(),
        }
    }

    fn __repr__(&self) -> String {
        let key_masked = self.inner.api_key.as_ref().map(|_| "***");
        format!(
            "PyResolvedModel(canonical_id='{}', provider='{}', api_key={:?}, credential_status='{}')",
            self.inner.canonical_id,
            self.inner.provider,
            key_masked,
            match self.inner.credential_status {
                CredentialStatus::Present => "present",
                CredentialStatus::NotRequired => "not_required",
                CredentialStatus::Missing => "missing",
            },
        )
    }
}
