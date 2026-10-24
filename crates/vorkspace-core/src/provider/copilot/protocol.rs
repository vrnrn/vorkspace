use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(super) const MODELS: [&str; 2] = ["gpt-6.1-sol", "gpt-6-luna"];
pub(super) fn model_id(value: &str) -> Result<&str, String> {
    let id = value.strip_prefix("copilot/").unwrap_or(value);
    MODELS
        .contains(&id)
        .then_some(id)
        .ok_or_else(|| format!("Unsupported Copilot model: {value}"))
}
fn input_block(block: &Value, role: &str) -> Result<Value, String> {
    Ok(match block["type"].as_str().unwrap_or_default() {
        "text" => {
            json!({"type":if role == "assistant" { "output_text" } else { "input_text" },"text":block["text"]})
        }
        "image" => {
            let source = &block["source"];
            let url = if source["type"] == "url" {
                source["url"].clone()
            } else {
                json!(format!(
                    "data:{};base64,{}",
                    source["media_type"]
                        .as_str()
                        .ok_or("Missing image media type")?,
                    source["data"].as_str().ok_or("Missing image data")?
                ))
            };
            json!({"type":"input_image","image_url":url})
        }
        "tool_reference" => {
            json!({"type":"input_text","text":format!("Available tool: {}",block["tool_name"].as_str().unwrap_or_default())})
        }
        kind => return Err(format!("Unsupported Copilot content: {kind}")),
    })
}
pub(super) fn to_responses(body: &Value, effort: Option<&str>) -> Result<Value, String> {
    let mut input = Vec::new();
    for message in body["messages"]
        .as_array()
        .ok_or("Messages must be an array")?
    {
        let role = message["role"].as_str().ok_or("Missing message role")?;
        let blocks = if let Some(text) = message["content"].as_str() {
            vec![json!({"type":"text","text":text})]
        } else {
            message["content"]
                .as_array()
                .ok_or("Invalid message content")?
                .clone()
        };
        let mut content = Vec::new();
        for block in blocks {
            let kind = block["type"].as_str().unwrap_or_default();
            if matches!(kind, "thinking" | "redacted_thinking") {
                continue;
            }
            if matches!(kind, "tool_use" | "tool_result") {
                if !content.is_empty() {
                    input.push(json!({"role":role,"content":std::mem::take(&mut content)}));
                }
                if kind == "tool_use" {
                    input.push(json!({"type":"function_call","call_id":block["id"],"name":block["name"],"arguments":block["input"].to_string()}));
                } else {
                    let mut output = if block["content"].is_string() {
                        block["content"].clone()
                    } else {
                        Value::Array(
                            block["content"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .map(|p| input_block(p, "user"))
                                .collect::<Result<_, _>>()?,
                        )
                    };
                    if block["is_error"] == true {
                        output = json!(format!(
                            "Tool error: {}",
                            output
                                .as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| output.to_string())
                        ));
                    }
                    input.push(json!({"type":"function_call_output","call_id":block["tool_use_id"],"output":output}));
                }
            } else {
                content.push(input_block(&block, role)?);
            }
        }
        if !content.is_empty() {
            input.push(json!({"role":role,"content":content}));
        }
    }
    let model = model_id(body["model"].as_str().ok_or("Missing model")?)?;
    let mut result =
        json!({"model":model,"input":input,"store":false,"contextTier":"long_context"});
    if !body["system"].is_null() {
        result["instructions"] =
            json!(body["system"]
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| {
                    body["system"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|p| p["type"] == "text")
                        .filter_map(|p| p["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                }));
    }
    if !body["max_tokens"].is_null() {
        result["max_output_tokens"] = body["max_tokens"].clone();
    }
    if let Some(tools) = body["tools"].as_array().filter(|v| !v.is_empty()) {
        result["tools"] = Value::Array(tools.iter().map(|tool| {
            if tool["input_schema"].is_null() { return Err("Unsupported Copilot tool schema".to_string()); }
            let mut value=json!({"type":"function","name":tool["name"],"parameters":tool["input_schema"],"strict":false});
            if !tool["description"].is_null() {value["description"]=tool["description"].clone();}
            Ok(value)
        }).collect::<Result<_,String>>()?);
    }
    let choice = &body["tool_choice"];
    if let Some(kind) = choice["type"].as_str() {
        result["tool_choice"] = match kind {
            "tool" => json!({"type":"function","name":choice["name"]}),
            "any" => json!("required"),
            _ => json!(kind),
        };
    }
    if choice["disable_parallel_tool_use"] == true {
        result["parallel_tool_calls"] = false.into();
    }
    if let Some(effort) = effort.or_else(|| body["output_config"]["effort"].as_str()) {
        result["reasoning"] = json!({"effort":effort});
    } else if body["thinking"]["type"] == "enabled" {
        let budget = body["thinking"]["budget_tokens"].as_u64().unwrap_or(0);
        result["reasoning"] = json!({"effort":if budget>=10000 {"high"} else if budget>=5000 {"medium"} else {"low"}});
    }
    let format = if body["output_config"]["format"].is_null() {
        &body["output_format"]
    } else {
        &body["output_config"]["format"]
    };
    if format["type"] == "json_schema" {
        result["text"] = json!({"format":{"type":"json_schema","name":"response","schema":format["schema"],"strict":false}});
    }
    Ok(result)
}
fn usage(value: &Value) -> Value {
    let cached = value["input_tokens_details"]["cached_tokens"]
        .as_u64()
        .unwrap_or(0);
    json!({"input_tokens":value["input_tokens"].as_u64().unwrap_or(0).saturating_sub(cached),"output_tokens":value["output_tokens"].as_u64().unwrap_or(0),"cache_read_input_tokens":cached})
}
fn stop(response: &Value, tools: bool) -> &'static str {
    if response["status"] == "incomplete" {
        "max_tokens"
    } else if tools {
        "tool_use"
    } else {
        "end_turn"
    }
}
pub(super) fn to_message(response: &Value, model: &str) -> Result<Value, String> {
    let mut content = Vec::new();
    for item in response["output"]
        .as_array()
        .ok_or("Missing Copilot output")?
    {
        match item["type"].as_str(){
            Some("function_call")=>content.push(json!({"type":"tool_use","id":item["call_id"],"name":item["name"],"input":serde_json::from_str::<Value>(item["arguments"].as_str().ok_or("Missing tool arguments")?).map_err(|_|"Invalid tool arguments")?})),
            Some("message")=>content.extend(item["content"].as_array().into_iter().flatten().filter(|p|p["type"]=="output_text").map(|p|json!({"type":"text","text":p["text"]}))),
            _=>{}
        }
    }
    Ok(
        json!({"id":format!("msg_{}",uuid::Uuid::new_v4()),"type":"message","role":"assistant","model":model,"stop_reason":stop(response,content.iter().any(|p|p["type"]=="tool_use")),"content":content,"stop_sequence":null,"usage":usage(&response["usage"])}),
    )
}
struct Block {
    index: usize,
    output: u64,
    open: bool,
    arguments: bool,
}
#[derive(Default)]
pub(super) struct Events {
    blocks: BTreeMap<(u64, u64), Block>,
    tools: bool,
    pub done: bool,
}
impl Events {
    pub fn start(model: &str) -> Value {
        json!({"type":"message_start","message":{"id":format!("msg_{}",uuid::Uuid::new_v4()),"type":"message","role":"assistant","model":model,"content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}})
    }
    pub fn push(&mut self, event: &Value) -> Result<Vec<Value>, String> {
        if self.done {
            return Err("Copilot sent data after completion".into());
        }
        let output = event["output_index"].as_u64().unwrap_or(0);
        let key = (output, event["content_index"].as_u64().unwrap_or(0));
        let mut result = Vec::new();
        match event["type"].as_str().unwrap_or_default() {
            "response.output_item.added" if event["item"]["type"] == "function_call" => {
                if self.blocks.contains_key(&key) {
                    return Err("Duplicate tool call index".into());
                }
                let index = self.blocks.len();
                self.blocks.insert(
                    key,
                    Block {
                        index,
                        output,
                        open: true,
                        arguments: false,
                    },
                );
                self.tools = true;
                result.push(json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":event["item"]["call_id"],"name":event["item"]["name"],"input":{}}}));
            }
            "response.output_text.delta" => {
                let index = self.blocks.len();
                let block=self.blocks.entry(key).or_insert_with(||{
                    result.push(json!({"type":"content_block_start","index":index,"content_block":{"type":"text","text":""}}));
                    Block{index,output,open:true,arguments:false}
                });
                if !block.open {
                    return Err("Text arrived after block closure".into());
                }
                result.push(json!({"type":"content_block_delta","index":block.index,"delta":{"type":"text_delta","text":event["delta"]}}));
            }
            "response.function_call_arguments.delta" => {
                let block = self
                    .blocks
                    .get_mut(&key)
                    .filter(|b| b.open)
                    .ok_or("Arguments arrived without a tool call")?;
                block.arguments = true;
                result.push(json!({"type":"content_block_delta","index":block.index,"delta":{"type":"input_json_delta","partial_json":event["delta"]}}));
            }
            "response.output_item.done" => {
                for block in self
                    .blocks
                    .values_mut()
                    .filter(|b| b.open && b.output == output)
                {
                    if event["item"]["type"] == "function_call" && !block.arguments {
                        result.push(json!({"type":"content_block_delta","index":block.index,"delta":{"type":"input_json_delta","partial_json":event["item"]["arguments"]}}));
                    }
                    block.open = false;
                    result.push(json!({"type":"content_block_stop","index":block.index}));
                }
            }
            "response.completed" | "response.incomplete" => {
                for block in self.blocks.values_mut().filter(|b| b.open) {
                    block.open = false;
                    result.push(json!({"type":"content_block_stop","index":block.index}));
                }
                result.push(json!({"type":"message_delta","delta":{"stop_reason":stop(&event["response"],self.tools),"stop_sequence":null},"usage":usage(&event["response"]["usage"])}));
                result.push(json!({"type":"message_stop"}));
                self.done = true;
            }
            "error" | "response.failed" => {
                return Err(event["message"]
                    .as_str()
                    .or_else(|| event["response"]["error"]["message"].as_str())
                    .unwrap_or("Copilot response failed")
                    .into());
            }
            _ => {}
        }
        Ok(result)
    }
}
