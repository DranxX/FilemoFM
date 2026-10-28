use crate::shared::constants::{APP_NAME, MAX_HTTP_BYTES};
use crate::shared::traits::{PermissionPolicy, ToolDispatcher};
use serde_json::{Value, json};
use std::io::{BufRead, Read, Write};
use std::sync::Arc;

const PROTOCOL_VERSION: &str = "2025-11-25";
const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const SESSION_ERROR: i64 = -32000;

pub fn run_stdio(
    tools: Arc<dyn ToolDispatcher>,
    policy: Arc<dyn PermissionPolicy>,
    schemas: Vec<Value>,
) -> Result<(), String> {
    if !policy.get().mcp_stdio || !policy.get().enabled {
        return Err("MCP stdio is not allowed in Settings".into());
    }
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let mut initialized = false;
    let mut ready = false;
    loop {
        let mut line = Vec::new();
        let count = Read::by_ref(&mut input)
            .take(MAX_HTTP_BYTES + 1)
            .read_until(b'\n', &mut line)
            .map_err(|_| "MCP input could not be read")?;
        if count == 0 {
            return Ok(());
        }
        if count as u64 > MAX_HTTP_BYTES {
            return Err("MCP request is too large".into());
        }
        let request = serde_json::from_slice::<Value>(&line);
        let response = match request {
            Err(_) => {
                json!({"jsonrpc":"2.0","id":null,"error":{"code":PARSE_ERROR,"message":"Invalid JSON"}})
            }
            Ok(request) => {
                let id = request.get("id").cloned();
                let method = request["method"].as_str().unwrap_or_default();
                let is_valid = request.is_object()
                    && request["jsonrpc"] == "2.0"
                    && !method.is_empty()
                    && id
                        .as_ref()
                        .is_none_or(|value| value.is_string() || value.as_i64().is_some());
                if !is_valid {
                    json!({"jsonrpc":"2.0","id":null,"error":{"code":INVALID_REQUEST,"message":"Invalid request"}})
                } else {
                    if id.is_none() {
                        if initialized && method == "notifications/initialized" {
                            ready = true;
                        }
                        continue;
                    }
                    let result: Result<Value, (i64, &str)> = if !policy.get().mcp_stdio
                        || !policy.get().enabled
                    {
                        Err((SESSION_ERROR, "MCP permission revoked"))
                    } else {
                        match method {
                            "initialize" if !initialized => {
                                let params = &request["params"];
                                if !params["protocolVersion"].is_string()
                                    || !params["capabilities"].is_object()
                                    || !params["clientInfo"]["name"].is_string()
                                    || !params["clientInfo"]["version"].is_string()
                                {
                                    Err((INVALID_PARAMS, "Invalid initialization parameters"))
                                } else {
                                    initialized = true;
                                    Ok(
                                        json!({"protocolVersion":PROTOCOL_VERSION,"capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":APP_NAME,"version":env!("CARGO_PKG_VERSION")}}),
                                    )
                                }
                            }
                            "ping" => Ok(json!({})),
                            _ if !ready => Err((SESSION_ERROR, "Initialize the MCP session first")),
                            "tools/list" => Ok(
                                json!({"tools":schemas.iter().map(|tool| json!({"name":tool["function"]["name"],"description":tool["function"]["description"],"inputSchema":tool["function"]["parameters"]})).collect::<Vec<_>>()}),
                            ),
                            "tools/call" => {
                                let name = request["params"]["name"].as_str().unwrap_or_default();
                                let arguments = request["params"]
                                    .get("arguments")
                                    .cloned()
                                    .unwrap_or_else(|| json!({}));
                                if !arguments.is_object()
                                    || !schemas
                                        .iter()
                                        .any(|schema| schema["function"]["name"] == name)
                                {
                                    Err((INVALID_PARAMS, "Tool or arguments unavailable"))
                                } else {
                                    Ok(match tools.dispatch(name, arguments) {
                                        Ok(value) => {
                                            json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false})
                                        }
                                        Err(error) => {
                                            json!({"content":[{"type":"text","text":error}],"isError":true})
                                        }
                                    })
                                }
                            }
                            _ => Err((METHOD_NOT_FOUND, "Method unavailable")),
                        }
                    };
                    match result {
                        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
                        Err((code, message)) => {
                            json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
                        }
                    }
                }
            }
        };
        let mut bytes = serde_json::to_vec(&response).map_err(|_| "MCP output failed")?;
        if bytes.len() as u64 > MAX_HTTP_BYTES {
            bytes = serde_json::to_vec(&json!({"jsonrpc":"2.0","id":response["id"],"error":{"code":SESSION_ERROR,"message":"Response exceeds limit"}})).map_err(|_| "MCP output failed")?;
        }
        bytes.push(b'\n');
        output
            .write_all(&bytes)
            .and_then(|_| output.flush())
            .map_err(|_| "MCP output failed")?;
    }
}
