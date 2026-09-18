use std::collections::BTreeMap;

use codex_protocol::models::ResponseItem;
use serde_json::Value;

use super::responses::ResponsesStreamEvent;

const UNKEYED_MESSAGE: &str = "__unkeyed_message__";

#[derive(Default)]
pub(super) struct CompletedMessageReconstructor {
    text_by_item: BTreeMap<String, String>,
    added_messages: BTreeMap<String, Value>,
}

impl CompletedMessageReconstructor {
    pub(super) fn observe(&mut self, event: &mut ResponsesStreamEvent) -> Vec<ResponseItem> {
        match event.kind() {
            "response.output_item.added" => {
                if let Some(item) = event.item.as_ref()
                    && item.get("type").and_then(Value::as_str) == Some("message")
                {
                    let key = item_key(event).unwrap_or_else(|| UNKEYED_MESSAGE.to_string());
                    self.added_messages.insert(key, item.clone());
                }
                Vec::new()
            }
            "response.output_text.delta" => {
                if let Some(delta) = event.delta.as_deref() {
                    let key = item_key(event)
                        .or_else(|| self.single_added_message_key())
                        .unwrap_or_else(|| UNKEYED_MESSAGE.to_string());
                    self.text_by_item.entry(key).or_default().push_str(delta);
                }
                Vec::new()
            }
            "response.output_item.done" => {
                if let Some(item) = event.item.as_mut() {
                    let key = item_key_from_item(item).or_else(|| self.single_text_key());
                    if let Some(key) = key
                        && let Some(text) = self.text_by_item.remove(&key)
                    {
                        backfill_message_text(item, &text);
                        self.added_messages.remove(&key);
                    }
                }
                Vec::new()
            }
            "response.completed" => self.take_missing_messages(),
            _ => Vec::new(),
        }
    }

    fn single_text_key(&self) -> Option<String> {
        (self.text_by_item.len() == 1)
            .then(|| self.text_by_item.keys().next().cloned())
            .flatten()
    }

    fn single_added_message_key(&self) -> Option<String> {
        (self.added_messages.len() == 1)
            .then(|| self.added_messages.keys().next().cloned())
            .flatten()
    }

    fn take_missing_messages(&mut self) -> Vec<ResponseItem> {
        std::mem::take(&mut self.text_by_item)
            .into_iter()
            .filter_map(|(key, text)| {
                if key == UNKEYED_MESSAGE && !self.added_messages.contains_key(&key) {
                    return None;
                }
                let mut item = self.added_messages.remove(&key).unwrap_or_else(|| {
                    let mut item = serde_json::json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [],
                    });
                    if key != UNKEYED_MESSAGE {
                        item["id"] = Value::String(key);
                    }
                    item
                });
                backfill_message_text(&mut item, &text);
                serde_json::from_value(item).ok()
            })
            .collect()
    }
}

fn item_key(event: &ResponsesStreamEvent) -> Option<String> {
    event
        .item_id
        .clone()
        .or_else(|| event.item.as_ref().and_then(item_key_from_item))
}

fn item_key_from_item(item: &Value) -> Option<String> {
    item.get("id").and_then(Value::as_str).map(str::to_string)
}

fn backfill_message_text(item: &mut Value, text: &str) {
    if text.is_empty() || item.get("type").and_then(Value::as_str) != Some("message") {
        return;
    }
    let Some(object) = item.as_object_mut() else {
        return;
    };
    let content = object
        .entry("content")
        .or_insert_with(|| Value::Array(Vec::new()));
    let Some(content) = content.as_array_mut() else {
        return;
    };
    if content.iter().any(|part| {
        part.get("type").and_then(Value::as_str) == Some("output_text")
            && part
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| !text.is_empty())
    }) {
        return;
    }
    if let Some(output_text) = content
        .iter_mut()
        .find(|part| part.get("type").and_then(Value::as_str) == Some("output_text"))
    {
        output_text["text"] = Value::String(text.to_string());
    } else {
        content.push(serde_json::json!({"type": "output_text", "text": text}));
    }
}
