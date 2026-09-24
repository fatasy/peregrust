//! MCP stdio adapter. All game semantics remain in the shared control API.
use std::io::{BufRead, Write};
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use serde_json::{Value, json};

#[derive(Parser)]
#[command(
    name = "peregrust mcp",
    about = "MCP stdio server for an existing control session"
)]
struct Options {
    #[arg(long)]
    session: PathBuf,
    #[arg(long, default_value_t = 10000, value_parser = clap::value_parser!(u64).range(1..=60000))]
    timeout_ms: u64,
}

struct Server {
    options: Options,
    initialized: bool,
    ready: bool,
    methods: serde_json::Map<String, Value>,
}

fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}

fn tool_result(mut response: Value) -> Value {
    let mut content = Vec::new();
    if let Some(capture) = response
        .get_mut("result")
        .and_then(|v| v.get_mut("capture"))
        && let Some(data) = capture.as_object_mut().and_then(|v| v.remove("data"))
    {
        content.push(json!({"type":"image","mimeType":"image/png","data":data}));
    }
    content.insert(0, json!({"type":"text","text":response.to_string()}));
    json!({"content":content,"structuredContent":response,"isError":response["ok"] != true})
}

impl Server {
    fn refresh(&mut self) -> Result<()> {
        let response = crate::control::request(
            &self.options.session,
            "control.describe",
            json!({}),
            self.options.timeout_ms,
        )?;
        self.methods = response["result"]["methods"]
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("control discovery failed: {response}"))?
            .clone();
        Ok(())
    }

    fn handle(&mut self, message: Value) -> Option<Value> {
        let id = message.get("id").cloned();
        let method = message.get("method").and_then(Value::as_str);
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || method.is_none()
            || id
                .as_ref()
                .is_some_and(|id| !(id.is_string() || id.is_number()))
        {
            return Some(rpc_error(
                id.unwrap_or(Value::Null),
                -32600,
                "Invalid Request",
            ));
        }
        let method = method.unwrap();
        let Some(id) = id else {
            if method == "notifications/initialized" && self.initialized {
                self.ready = true;
            }
            // Unknown notifications have no response, including cancellation.
            return None;
        };
        let params = message.get("params").cloned().unwrap_or_else(|| json!({}));
        if !params.is_object() {
            return Some(rpc_error(id, -32602, "params must be an object"));
        }
        if method == "initialize" {
            if self.initialized {
                return Some(rpc_error(id, -32600, "Already initialized"));
            }
            if !params["protocolVersion"].is_string()
                || !params["capabilities"].is_object()
                || !params["clientInfo"].is_object()
            {
                return Some(rpc_error(
                    id,
                    -32602,
                    "initialize needs protocolVersion, capabilities and clientInfo",
                ));
            }
            self.initialized = true;
            let requested = params["protocolVersion"].as_str().unwrap();
            let version = match requested {
                "2025-06-18" | "2025-03-26" => requested,
                _ => "2025-11-25",
            };
            return Some(json!({"jsonrpc":"2.0","id":id,"result":{
                "protocolVersion":version,"capabilities":{"tools":{}},
                "serverInfo":{"name":"peregrust","version":env!("CARGO_PKG_VERSION")},
                "instructions":"Use control_describe to discover game operations. Query bounded fields; IDs last for the current game session. Actions change live state. Captures return image content. Requests are serialized with a bounded timeout."
            }}));
        }
        if method == "ping" {
            return Some(json!({"jsonrpc":"2.0","id":id,"result":{}}));
        }
        if !self.ready {
            return Some(rpc_error(id, -32002, "Complete initialization first"));
        }
        let result = match method {
            "tools/list" => {
                if params.get("cursor").is_some() {
                    return Some(rpc_error(
                        id,
                        -32602,
                        "This server returns all tools in one page",
                    ));
                }
                if let Err(error) = self.refresh() {
                    return Some(rpc_error(id, -32603, &error.to_string()));
                }
                let tools: Vec<_> = self.methods.iter().map(|(name, spec)| json!({
                    "name":name.replace('.', "_"),"description":spec["description"],"inputSchema":spec["inputSchema"]
                })).collect();
                json!({"tools":tools})
            }
            "tools/call" => {
                let Some(name) = params["name"].as_str() else {
                    return Some(rpc_error(id, -32602, "name is required"));
                };
                if self.methods.is_empty()
                    && let Err(error) = self.refresh()
                {
                    return Some(rpc_error(id, -32603, &error.to_string()));
                }
                let Some(operation) = self
                    .methods
                    .keys()
                    .find(|method| method.replace('.', "_") == name)
                    .cloned()
                else {
                    return Some(rpc_error(id, -32602, "Unknown tool"));
                };
                let arguments = params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                if !arguments.is_object() {
                    return Some(rpc_error(id, -32602, "arguments must be an object"));
                }
                let response = crate::control::request(&self.options.session, &operation, arguments, self.options.timeout_ms)
                    .unwrap_or_else(|error| json!({"ok":false,"error":{"code":"CONTROL_ERROR","message":error.to_string()}}));
                tool_result(response)
            }
            _ => return Some(rpc_error(id, -32601, "Method not found")),
        };
        Some(json!({"jsonrpc":"2.0","id":id,"result":result}))
    }
}

pub fn run() -> Result<i32> {
    let options = match Options::try_parse_from(
        std::iter::once(std::ffi::OsString::from("peregrust mcp"))
            .chain(std::env::args_os().skip(2)),
    ) {
        Ok(value) => value,
        Err(error) => {
            let code = error.exit_code();
            error.print()?;
            return Ok(code);
        }
    };
    let mut server = Server {
        options,
        initialized: false,
        ready: false,
        methods: Default::default(),
    };
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    loop {
        let mut line = Vec::new();
        // Enforce a bounded line without allowing unbounded stdin allocations.
        let mut oversized = false;
        loop {
            let buffer = input.fill_buf()?;
            if buffer.is_empty() {
                break;
            }
            let end = buffer.iter().position(|byte| *byte == b'\n').map(|i| i + 1);
            let count = end.unwrap_or(buffer.len());
            if line.len() + count > 1024 * 1024 {
                oversized = true;
            }
            if !oversized {
                line.extend_from_slice(&buffer[..count]);
            }
            input.consume(count);
            if end.is_some() {
                break;
            }
        }
        if line.is_empty() && !oversized {
            break;
        }
        let response = if oversized {
            Some(rpc_error(Value::Null, -32600, "Request exceeds 1 MiB"))
        } else {
            match serde_json::from_slice(&line) {
                Ok(value) => server.handle(value),
                Err(_) => Some(rpc_error(Value::Null, -32700, "Parse error")),
            }
        };
        if let Some(response) = response {
            serde_json::to_writer(&mut output, &response)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_and_protocol_errors_do_not_need_a_game() {
        let mut server = Server {
            options: Options {
                session: "unused".into(),
                timeout_ms: 1000,
            },
            initialized: false,
            ready: false,
            methods: Default::default(),
        };
        assert_eq!(
            server
                .handle(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
                .unwrap()["error"]["code"],
            -32002
        );
        let init = server.handle(json!({"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"future","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).unwrap();
        assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
        assert!(
            server
                .handle(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
                .is_none()
        );
        assert_eq!(
            server
                .handle(json!({"jsonrpc":"2.0","id":3,"method":"unknown"}))
                .unwrap()["error"]["code"],
            -32601
        );
    }
    #[test]
    fn image_payload_is_not_duplicated_into_text_or_structured_content() {
        let output = tool_result(
            json!({"ok":true,"frame":1,"result":{"capture":{"data":"pixels","mimeType":"image/png"}}}),
        );
        assert_eq!(output["content"][1]["data"], "pixels");
        assert!(
            !output["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("pixels")
        );
        assert!(
            output["structuredContent"]["result"]["capture"]
                .get("data")
                .is_none()
        );
    }
}
